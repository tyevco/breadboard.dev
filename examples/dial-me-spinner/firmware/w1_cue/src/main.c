/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * W1 firmware (Phase 2 plan section 2 W1 and milestone W-fw; ADR-009 D6, Amendment 2, decision
 * D-W1): the POF blanking rule R8 of dial-me-spinner docs/firmware.md section 12.4 (at 6854d25)
 * on the charging cue of a dead battery on USB. The firmware's part of the case:
 *
 *   Boot:  LED_PWR_EN (P1.04), LED_DIN_MCU (P0.06) and OLED_VCC_EN (P1.02) are outputs driven low
 *          (firmware.md section 1.3's default column); POFCON = POF | THRESHOLD V28 with the
 *          POFWARN interrupt (nrfx_power). V28 (2.8 V) is the highest normal-voltage code: the
 *          nRF52840 has no 2.9 V code (ADR-009 Context, Q12), so firmware.md's 2.9 V cannot be set.
 *   Cue:   W1_CUE_DELAY_MS after main() starts: the blanking starts (POFCON.POF = 0 and the
 *          POFWARN interrupt off, nrfx_power_pof_disable()), then LED_PWR_EN rises (U2 TPS22917
 *          inrush: +3V3 dips), then the blanking timer starts. When it expires the firmware
 *          re-enables POFCON at V28 (nrfx_power_pof_enable()). The nRF52840 raises POFWARN at
 *          enable if VDD is already below the threshold (PS v1.1; ADR-009 D6), so a dip still in
 *          progress when the blanking ends sheds the cue: the W1 firmware's choice of how to
 *          implement R8 (plan section 2 W1), not a statement of firmware.md.
 *          W1_DIN_DELAY_MS after LED_PWR_EN, P0.06 rises: it stands in for the first frame of the
 *          cue (the I2S drives no pin in Renode), at least 10 ms after VLED is switched on
 *          (firmware.md section 6.4, LED-DATA-V4), as in E1's led_rail.
 *   POF:   POFWARN outside the blanking sheds the cue at once (firmware.md section 12.4: "LEDs
 *          off"), in the section 6.4 order: P0.06 low, then LED_PWR_EN low. The cue stays off.
 *          SHIP after 100 ms needs the charger's I2C, which Phase 2 does not model (plan
 *          section 3 Out), so it is not implemented.
 *
 * CONFIG_W1_BLANK_MS is the one switch between the four images (milestone W-fw): 10 (w1_rule.elf,
 * firmware.md's rule), 4 (w1_blank4.elf: the window-midpoint negative control, the whole ms
 * nearest the midpoint of the stress corner's measured 2.94 V window, 3.255-4.089 ms after
 * LED_PWR_EN, ADR-009 Amendment 2), 5 (w1_blank5.elf, the 5 ms report) and 0 (w1_blank_off.elf:
 * no blanking, POFCON stays enabled through the edge; the second negative control).
 *
 * The blanking is timed by TIMER1 at 1 MHz (Zephyr's counter API), started right after the
 * LED_PWR_EN write, so it ends W1_BLANK_MS plus a few microseconds after the edge. The system
 * timer (RTC1, 30.5 us ticks, one tick added to every relative timeout) would end a 4 ms
 * blanking 4.03-4.06 ms after the edge, within three lattice cells of the stress window's end.
 * TIMER1 runs only during the blanking.
 *
 * The console (UARTE0 TX on P1.00, the tap of ADR-008 D8) prints a boot line and one line per
 * POFWARN. ILIM and SYS_REG of the BQ25180 are the bench's: the firmware has no I2C (plan
 * section 2 W1, recorded deviation).
 */
#include <zephyr/kernel.h>
#include <zephyr/drivers/counter.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/irq.h>
#include <zephyr/sys/printk.h>
#include <nrfx_power.h>

#define BLANK_US ((uint32_t)CONFIG_W1_BLANK_MS * 1000U)

static const struct gpio_dt_spec led_pwr_en =
	GPIO_DT_SPEC_GET(DT_NODELABEL(board_signals), led_pwr_en_gpios);
static const struct gpio_dt_spec led_din =
	GPIO_DT_SPEC_GET(DT_NODELABEL(board_signals), led_din_gpios);
static const struct gpio_dt_spec oled_vcc_en =
	GPIO_DT_SPEC_GET(DT_NODELABEL(board_signals), oled_vcc_en_gpios);
static const struct device *const blank_timer = DEVICE_DT_GET(DT_NODELABEL(timer1));

static volatile bool cue_on;
static volatile uint32_t pofwarns;
static K_SEM_DEFINE(pof_sem, 0, 1);

/* POFWARN (the POWER_CLOCK interrupt, through nrfx_power): shed the cue. */
static void pofwarn(void)
{
	pofwarns++;
	if (cue_on) {
		(void)gpio_pin_set_dt(&led_din, 0);
		(void)gpio_pin_set_dt(&led_pwr_en, 0);
		cue_on = false;
	}
	k_sem_give(&pof_sem);
}

static const nrfx_power_pofwarn_config_t pof_config = {
	.handler = pofwarn,
	.thr = NRF_POWER_POFTHR_V28,
};

/* TIMER1 compare: the blanking ends; POFCON is enabled again at V28. */
static void blanking_end(const struct device *dev, uint8_t chan, uint32_t ticks, void *user_data)
{
	ARG_UNUSED(chan);
	ARG_UNUSED(ticks);
	ARG_UNUSED(user_data);
	nrfx_power_pof_enable(&pof_config);
	(void)counter_stop(dev);
}

static struct counter_alarm_cfg blank_alarm = {
	.callback = blanking_end,
	.flags = 0, /* relative to the counter when the alarm is set */
};

int main(void)
{
	if (!gpio_is_ready_dt(&led_pwr_en) || !gpio_is_ready_dt(&led_din) ||
	    !gpio_is_ready_dt(&oled_vcc_en) || !device_is_ready(blank_timer)) {
		return 0;
	}
	(void)gpio_pin_configure_dt(&led_pwr_en, GPIO_OUTPUT_INACTIVE);
	(void)gpio_pin_configure_dt(&led_din, GPIO_OUTPUT_INACTIVE);
	(void)gpio_pin_configure_dt(&oled_vcc_en, GPIO_OUTPUT_INACTIVE);

	nrfx_power_pof_init(&pof_config);
	nrfx_power_pof_enable(&pof_config);
	blank_alarm.ticks = counter_us_to_ticks(blank_timer, BLANK_US);
	printk("w1: boot pofcon=V28 blank=%u ms\n", (unsigned int)CONFIG_W1_BLANK_MS);

	k_msleep(CONFIG_W1_CUE_DELAY_MS);

	/* The charging cue: LED_PWR_EN rises with POFWARN blanked for W1_BLANK_MS. */
	if (BLANK_US > 0U) {
		nrfx_power_pof_disable();
	}
	cue_on = true;
	(void)gpio_pin_set_dt(&led_pwr_en, 1);
	if (BLANK_US > 0U) {
		(void)counter_start(blank_timer);
		(void)counter_set_channel_alarm(blank_timer, 0, &blank_alarm);
	}

	/* The first frame, at least 10 ms after VLED is switched on, unless the cue was shed. */
	k_msleep(CONFIG_W1_DIN_DELAY_MS);
	unsigned int key = irq_lock();

	if (cue_on) {
		(void)gpio_pin_set_dt(&led_din, 1);
	}
	irq_unlock(key);

	for (;;) {
		(void)k_sem_take(&pof_sem, K_FOREVER);
		printk("w1: pofwarn n=%u cue=%s\n", (unsigned int)pofwarns, cue_on ? "on" : "shed");
	}
	return 0;
}
