/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * E2 firmware (ADR-008 D7-D8, Phase 1 plan section 2 E2, M8): the battery reading and the
 * low-battery LED shed of dial-me-spinner docs/firmware.md section 12.4 (at 6854d25), on the
 * board subset in ../boards/dial/dial_me_spinner plus this application's overlay (the SAADC
 * channel, the vsys-sense divider and the UARTE0 log of firmware.md section 3.1).
 *
 * The reading (firmware.md section 12.4):
 *   VSYS_SENSE = VSYS / 3.2 on AIN0 (P0.02); gain 1/6, the internal 0.6 V reference (3.6 V full
 *   scale), 12 bits, 8x oversampling, TACQ 40 us. Zephyr's SAADC driver runs one channel with
 *   oversampling in nrfx's simple mode, which sets BURST: one SAMPLE task, 8 acquisitions.
 *   VSYS = code x 3.6 V / 4096 x 3.2 = code x 2.8125 mV (the divider's full-ohms / output-ohms).
 *   VBAT_est = 0.979 x reading (rule R1', VREAD_GAIN). No production calibration is stored
 *   (cal/vsys_gain = 1) and the rested-OCV correction is not applied: the plan's E2 compares
 *   0.979 x code x 2.8125 mV with the threshold.
 *
 * The policy (firmware.md section 12.4, the "< 3.53 V" row): when VBAT_est < 3.53 V, the LEDs are
 * shed with E1's off sequence (firmware.md section 6.4: P0.06 low, then LED_PWR_EN low); they
 * come back only at VBAT_est >= 3.58 V (50 mV of hysteresis upward), and never sooner than 30 ms
 * after the shed (the QOD time, LSW-2). The red cue, the quiet sounds and the other rows are out
 * of E2.
 *
 * Timing: one reading every CONFIG_VSENSE_PERIOD_MS on absolute RTC deadlines (k_sleep, never a
 * busy-wait: ADR-008 D1), the first right after boot. firmware.md's power_thread reads on a 1 s
 * tick (vsense.elf); vsense_50ms.elf reads every 50 ms, the suite's shortest burst interval
 * (hardware/sim test_sample_interval), for the sample-rate check of plan section 2 E2.
 *
 * Every reading prints one line on UARTE0 (a log record, ADR-008 D8):
 *   vsense: n=<k> code=<code> vbat_est=<uV> uV leds=<on|off>
 * and the shed and the restore each print "vsense: leds off ..." / "vsense: leds on ...".
 */
#include <zephyr/kernel.h>
#include <zephyr/drivers/adc.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/drivers/regulator.h>
#include <zephyr/sys/printk.h>

#define VSENSE_NODE DT_NODELABEL(vsys_sense)

/* firmware.md section 3.1: full-ohms 3.2 MOhm, output-ohms 1 MOhm (R1 2.2 MOhm over R2 1 MOhm). */
#define FULL_OHMS   DT_PROP(VSENSE_NODE, full_ohms)
#define OUTPUT_OHMS DT_PROP(VSENSE_NODE, output_ohms)

/* firmware.md section 12.4, in uV: VREAD_GAIN 0.979 (ppm), the LED shed and its hysteresis. */
#define VREAD_GAIN_PPM   979000LL
#define SHED_UV          3530000LL
#define RESTORE_UV       (SHED_UV + 50000LL)
#define LED_OFF_MIN_MS   30

static const struct adc_dt_spec vsense = ADC_DT_SPEC_GET(VSENSE_NODE);
static const struct device *const vled = DEVICE_DT_GET(DT_NODELABEL(vled));
static const struct gpio_dt_spec led_din =
	GPIO_DT_SPEC_GET(DT_NODELABEL(board_signals), led_din_gpios);

/*
 * VBAT_est in units of 1e-12 V (pV), exact: code x 3.6 V / 4096 x FULL / OUTPUT x 0.979.
 * One LSB at the pin is 3.6e12 / 4096 = 878 906 250 pV; the divider ratio is taken in units of
 * 100 kOhm (32 / 10), so every step is an exact integer and nothing overflows (code <= 4095).
 */
BUILD_ASSERT(FULL_OHMS % 100000 == 0 && OUTPUT_OHMS % 100000 == 0, "divider in 100 kOhm units");

static int64_t vbat_est_pv(int32_t code)
{
	int64_t pv = (int64_t)code * 878906250LL;

	pv = pv * (FULL_OHMS / 100000) / (OUTPUT_OHMS / 100000);
	return pv * (VREAD_GAIN_PPM / 1000) / 1000;
}

static void rail_on(void)
{
	(void)regulator_enable(vled);  /* LED_PWR_EN high; sleeps off-on-delay-us (E1) */
	(void)gpio_pin_set_dt(&led_din, 1);
}

static void rail_off(void)
{
	(void)gpio_pin_configure_dt(&led_din, GPIO_OUTPUT_LOW);
	(void)regulator_disable(vled);
}

int main(void)
{
	int16_t sample;
	struct adc_sequence seq = {
		.buffer = &sample,
		.buffer_size = sizeof(sample),
	};
	bool leds = false;
	int64_t shed_at_ms = 0;
	int64_t next_ms;
	uint32_t n = 0;

	if (!adc_is_ready_dt(&vsense) || !device_is_ready(vled) || !gpio_is_ready_dt(&led_din)) {
		printk("vsense: device not ready\n");
		return 0;
	}
	/* DIN never floats on R41 alone: an output driven low from the start. */
	(void)gpio_pin_configure_dt(&led_din, GPIO_OUTPUT_LOW);
	if (adc_channel_setup_dt(&vsense) < 0 || adc_sequence_init_dt(&vsense, &seq) < 0) {
		printk("vsense: adc setup failed\n");
		return 0;
	}

	next_ms = k_uptime_get();
	for (;;) {
		int err = adc_read_dt(&vsense, &seq);
		int32_t code = err == 0 ? sample : -1;
		int64_t est_pv = vbat_est_pv(code < 0 ? 0 : code);
		int64_t est_uv = est_pv / 1000000LL;

		if (err != 0) {
			printk("vsense: n=%u adc_read error %d\n", n, err);
		} else if (n == 0) {
			/* At boot the LEDs come on only above the shed threshold. */
			if (est_pv >= SHED_UV * 1000000LL) {
				rail_on();
				leds = true;
			}
		} else if (leds && est_pv < SHED_UV * 1000000LL) {
			rail_off();
			leds = false;
			shed_at_ms = k_uptime_get();
			printk("vsense: leds off (vbat_est %d uV < %d uV)\n", (int)est_uv, (int)SHED_UV);
		} else if (!leds && est_pv >= RESTORE_UV * 1000000LL &&
			   k_uptime_get() - shed_at_ms >= LED_OFF_MIN_MS) {
			rail_on();
			leds = true;
			printk("vsense: leds on (vbat_est %d uV >= %d uV)\n", (int)est_uv,
			       (int)RESTORE_UV);
		}
		printk("vsense: n=%u code=%d vbat_est=%d uV leds=%s\n", n, (int)code, (int)est_uv,
		       leds ? "on" : "off");
		n++;
		next_ms += CONFIG_VSENSE_PERIOD_MS;
		k_sleep(K_TIMEOUT_ABS_MS(next_ms));
	}
	return 0;
}
