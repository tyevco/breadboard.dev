/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * E3 firmware (ADR-008 D5, D9, Amendment 9; Phase 1 plan section 2 E3, M10): the detent homing
 * H0-H5 of dial-me-spinner docs/firmware.md section 5.4 (at 6854d25), on the board subset in
 * ../boards/dial/dial_me_spinner plus this application's overlay (the detent-stepper node, the
 * SAADC channel of the VSYS reading and the UARTE0 log of firmware.md section 3.1).
 *
 * The drive (firmware.md section 5.3):
 *   PWM0 through nrfx directly, 16 MHz, COUNTERTOP 640 (25 kHz), Individual decoder: channel 0 =
 *   MOT_A_IN1 (P0.26), 1 = MOT_A_IN2 (P0.05), 2 = MOT_B_IN1 (P0.07), 3 = MOT_B_IN2 (P0.11).
 *   8 microsteps per full step: coil A ~ cos(theta), coil B ~ sin(theta), theta = 11.25 deg x u.
 *   Slow decay: for a positive coil current IN1 is held high and IN2 carries the PWM, low (drive)
 *   for d x 640 ticks from the period start, then high (brake, both low sides on); swapped for a
 *   negative current. Each value's polarity bit is 0 (the period starts low): COMP 0 holds a
 *   channel high all period, COMP = d x 640 gives the drive part first, as the dial-me-spinner
 *   suite's microstep_ctrl does.
 *   nrfx_pwm_complex_playback with two ping-pong sequences, each one full step (8 microsteps x 4
 *   channels); REFRESH sets the speed, 25000 / (8 (REFRESH + 1)) PPS: 446 PPS (REFRESH 6), 313
 *   (9), 80 (38). The SEQEND interrupt refills the sequence whose last value just loaded with the
 *   next segment (a full step, a hold or the brake) and that segment's REFRESH; the homing state
 *   machine decides that segment there, reading HOME, one segment ahead of what plays.
 *   Amplitude, MD-3: d = I x (20 Ohm x (1 + 0.00393 x (max(T, 25 C) - 25 C)) + 0.33 Ohm) / VSYS,
 *   clamped to 1, with T the nRF die temperature (TEMP) and VSYS the idle reading (the SAADC on
 *   VSYS_SENSE, code x 2.8125 mV, as E2 reads it). Run current I = 160 mA; the final stall 80 mA.
 *
 * The homing procedure (firmware.md section 5.4, the unlearned defaults: no factory test 19 has
 * run, so s_rel,L = 333 and s_ast,L = 263 steps from the park stop):
 *   H0 wake: nSLEEP high, wait 1 ms (k_sleep), energise the stored phase (microstep 0) for
 *      10.24 ms at run current.
 *   H1 back-off (only if HOME reads asserted): toward click, run current, 313 PPS, until HOME
 *      releases; limit s_rel,L + 24 + 20 = 377 steps, else a failed attempt.
 *   H2 approach: toward park, run current, 4 steps at 313 PPS then 446 PPS, until the released ->
 *      asserted edge; limit 500 + 40 = 540 steps, else a failed attempt.
 *   H3 run-in: toward park, run current, 446 PPS, n3 = s_ast,L - 34 - 10 - 20 = 199 steps.
 *   H4 final stall: 80 PPS, 50 % current, the remaining maximum + 20 of overdrive = 128 steps
 *      (the switch from 446 to 80 PPS is at a full-step boundary).
 *   H5 confirm: HOME asserted -> back off 4 steps at run current (313 PPS), x = x_park =
 *      stop - 4 steps; SETTLE (hold 20.16 ms); brake (IN1 = IN2 = 1 on both bridges, 3.12 ms,
 *      firmware.md section 5.2, MD-1); STOP (the pins go back to their GPIO level, low: coast);
 *      nSLEEP low (IDLE). HOME not asserted -> a failed attempt.
 *   A failed attempt brakes, sleeps and retries from H0 with every travel phase at 313 PPS; after
 *   3 failures FAULT (E_DET_HOME). HOME is read when a segment is decided, so the confirm of H5
 *   reads HOME while the last stall step plays (the stall itself is over by then).
 *   Not here (out of E3): the NVS write of state/carriage (the NVMC stand-in has no write timing,
 *   ADR-008 Amendment 9), the speed ramp table of section 5.3, learned edges, edge windows.
 *
 * homing_step.elf (CONFIG_HOMING_STEP_TEST): H0, then one full step toward click at 446 PPS, then
 * SETTLE, brake and sleep: the drive of the suite's test_firmware_microstep_pwm.
 *
 * Every phase prints one line on UARTE0 (a log record, ADR-008 D8), "homing: ...". Timing is
 * k_sleep and the PWM (never a busy-wait: ADR-008 D1).
 */
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#include <zephyr/kernel.h>
#include <zephyr/drivers/adc.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/irq.h>
#include <zephyr/sys/printk.h>

#include <hal/nrf_gpio.h>
#include <hal/nrf_temp.h>
#include <nrfx_pwm.h>

#define DETENT_NODE DT_NODELABEL(detent)
#define VSENSE_NODE DT_NODELABEL(vsys_sense)

/* firmware.md section 1.3 / the pwm0_default pinctrl group: PWM0 channels 0-3. */
#define PIN_A_IN1 NRF_GPIO_PIN_MAP(0, 26)
#define PIN_A_IN2 NRF_GPIO_PIN_MAP(0, 5)
#define PIN_B_IN1 NRF_GPIO_PIN_MAP(0, 7)
#define PIN_B_IN2 NRF_GPIO_PIN_MAP(0, 11)

#define TOP 640u                       /* 16 MHz / 640 = 25 kHz */
#define R_446 6u                       /* REFRESH: 25000 / (8 x 7) = 446 PPS */
#define R_313 9u                       /* 25000 / (8 x 10) = 313 PPS */
#define R_80 38u                       /* 25000 / (8 x 39) = 80 PPS */
#define R_H0 31u                       /* 8 x 32 periods = 10.24 ms */
#define R_SETTLE 62u                   /* 8 x 63 periods = 20.16 ms */
#define R_BRAKE 10u                    /* 7 x 11 + 1 periods = 3.12 ms (STOP at the last value) */

#define I_RUN_UA 160000LL              /* firmware.md section 5.3 run current */
#define I_STALL_UA 80000LL             /* 50 % for the final stall */
#define R_COIL_UOHM (DT_PROP(DETENT_NODE, coil_milliohms) * 1000LL)  /* 20 Ohm at 25 C */
#define R_DRV_UOHM 330000LL            /* + 0.33 Ohm (rDS(on) HS + LS, max) */
#define STROKE DT_PROP(DETENT_NODE, stroke_steps)

/* firmware.md section 5.4, the unlearned defaults. */
#define S_REL_L 333
#define S_AST_L 263
#define H1_LIMIT (S_REL_L + 24 + 20)   /* 377 */
#define H2_LIMIT (STROKE + 40)         /* 540 */
#define H3_STEPS (S_AST_L - 34 - 10 - 20) /* 199 */
#define H4_STEPS 128
#define BACKOFF_STEPS 4
#define ATTEMPTS 3

/* firmware.md section 3.1, E2: full-ohms 3.2 MOhm, output-ohms 1 MOhm. */
#define FULL_OHMS DT_PROP(VSENSE_NODE, full_ohms)
#define OUTPUT_OHMS DT_PROP(VSENSE_NODE, output_ohms)

static const struct gpio_dt_spec nsleep = GPIO_DT_SPEC_GET(DETENT_NODE, nsleep_gpios);
static const struct gpio_dt_spec home = GPIO_DT_SPEC_GET(DETENT_NODE, home_gpios);
static const struct adc_dt_spec vsense = ADC_DT_SPEC_GET(VSENSE_NODE);

static nrfx_pwm_t pwm = NRFX_PWM_INSTANCE(NRF_PWM0);

/* cos(11.25 degrees x k) x 10000, k = 0..8 */
static const int32_t cos_tab[9] = { 10000, 9808, 9239, 8315, 7071, 5556, 3827, 1951, 0 };

enum seg_kind { SEG_NONE, SEG_STEP, SEG_HOLD, SEG_BRAKE };

struct seg {
	uint8_t kind;
	int8_t dir;          /* +1: toward the click end (theta increasing); -1: toward park */
	uint16_t refresh;
	uint16_t amp;        /* ticks of 640 at the peak of the sine */
};

enum phase { P_H0, P_H1, P_H2, P_H3, P_H4, P_H5, P_BACK, P_SETTLE, P_BRAKE, P_DONE };

static nrf_pwm_values_individual_t buf[2][8];
static nrf_pwm_sequence_t seq[2] = {
	{ .values.p_individual = buf[0], .length = 32, .repeats = 0, .end_delay = 0 },
	{ .values.p_individual = buf[1], .length = 32, .repeats = 0, .end_delay = 0 },
};
static uint8_t kind_of[2];
static int32_t u;              /* the microstep the field points at (unwrapped) */
static uint16_t amp_run, amp_stall;
static bool slow;              /* a retry: every travel phase at 313 PPS */
static enum phase ph;
static int n_steps;            /* steps of the current phase */
static bool seen_released;
static bool failed;
static K_SEM_DEFINE(stopped, 0, 1);

static int32_t cos_u(int32_t k)  /* cos(11.25 k degrees) x 10000 */
{
	k &= 31;
	if (k <= 8) {
		return cos_tab[k];
	}
	if (k <= 16) {
		return -cos_tab[16 - k];
	}
	if (k <= 24) {
		return -cos_tab[k - 16];
	}
	return cos_tab[32 - k];
}

/* One coil: c = the sine value x 10000 at amplitude amp. */
static void coil(uint16_t *in1, uint16_t *in2, int32_t c, uint32_t amp)
{
	uint16_t a = (uint16_t)((amp * (uint32_t)abs(c) + 5000u) / 10000u);

	if (c >= 0) {
		*in1 = 0;   /* COMP 0, polarity 0: high all period */
		*in2 = a;   /* low (drive) for a ticks, then high (brake) */
	} else {
		*in1 = a;
		*in2 = 0;
	}
}

static void fill(int n, const struct seg *s)
{
	for (int j = 0; j < 8; j++) {
		nrf_pwm_values_individual_t *v = &buf[n][j];

		if (s->kind == SEG_STEP) {
			u += s->dir;
		}
		if (s->kind == SEG_BRAKE) {
			v->channel_0 = v->channel_1 = v->channel_2 = v->channel_3 = 0;
		} else {
			coil(&v->channel_0, &v->channel_1, cos_u(u), s->amp);
			coil(&v->channel_2, &v->channel_3, cos_u(u - 8), s->amp);  /* sin */
		}
	}
	seq[n].repeats = s->refresh;
	kind_of[n] = s->kind;
}

static bool home_asserted(void)
{
	return gpio_pin_get_dt(&home) == 1;  /* active low: HOME_N low = magnet present */
}

static struct seg step(int8_t dir, uint16_t refresh, uint16_t amp)
{
	n_steps++;
	return (struct seg){ SEG_STEP, dir, refresh, amp };
}

static struct seg fail(const char *what)
{
	printk("homing: %s (attempt failed)\n", what);
	failed = true;
	ph = P_DONE;
	return (struct seg){ SEG_BRAKE, 0, R_BRAKE, 0 };
}

static void enter(enum phase p)
{
	ph = p;
	n_steps = 0;
}

/* The next segment (called from the SEQEND interrupt, and twice before the start). */
static struct seg next_seg(void)
{
	for (;;) {
		switch (ph) {
		case P_H0:
			if (n_steps == 0) {
				n_steps = 1;
				printk("homing: H0 energise u=%d for 10.24 ms (d %u/640)\n", (int)u,
				       amp_run);
				return (struct seg){ SEG_HOLD, 0, R_H0, amp_run };
			}
#ifdef CONFIG_HOMING_STEP_TEST
			printk("homing: step test: one full step toward click at 446 PPS\n");
			ph = P_BACK;
			n_steps = 0;
			return step(+1, R_446, amp_run);
#else
			if (home_asserted()) {
				printk("homing: H1 back-off: HOME asserted\n");
				enter(P_H1);
			} else {
				printk("homing: H1 skipped: HOME released; H2 approach\n");
				enter(P_H2);
			}
			continue;
#endif
		case P_H1:
			if (!home_asserted()) {
				printk("homing: H1 done: HOME released after %d steps; H2 approach\n",
				       n_steps);
				enter(P_H2);
				seen_released = true;
				continue;
			}
			if (n_steps >= H1_LIMIT) {
				return fail("H1 failed: HOME still asserted after 377 steps");
			}
			return step(+1, R_313, amp_run);
		case P_H2:
			if (!home_asserted()) {
				seen_released = true;
			} else if (seen_released) {
				printk("homing: H2 done: HOME asserted after %d steps; H3 run-in %d steps\n",
				       n_steps, H3_STEPS);
				enter(P_H3);
				continue;
			}
			if (n_steps >= H2_LIMIT) {
				return fail("H2 failed: no HOME edge in 540 steps");
			}
			return step(-1, (slow || n_steps < 4) ? R_313 : R_446, amp_run);
		case P_H3:
			if (n_steps >= H3_STEPS) {
				printk("homing: H3 done; H4 final stall %d steps at 80 PPS, 50 %% current "
				       "(d %u/640)\n", H4_STEPS, amp_stall);
				enter(P_H4);
				continue;
			}
			return step(-1, slow ? R_313 : R_446, amp_run);
		case P_H4:
			if (n_steps >= H4_STEPS) {
				enter(P_H5);
				continue;
			}
			return step(-1, R_80, amp_stall);
		case P_H5:
			if (!home_asserted()) {
				return fail("H5 failed: HOME not asserted after the stall");
			}
			printk("homing: H5 confirm: HOME asserted; back off %d steps\n",
			       BACKOFF_STEPS);
			enter(P_BACK);
			continue;
		case P_BACK:
#ifdef CONFIG_HOMING_STEP_TEST
			if (n_steps >= 1) {
#else
			if (n_steps >= BACKOFF_STEPS) {
#endif
				printk("homing: SETTLE 20.16 ms at u=%d\n", (int)u);
				ph = P_SETTLE;
				return (struct seg){ SEG_HOLD, 0, R_SETTLE, amp_run };
			}
			return step(+1, R_313, amp_run);
		case P_SETTLE:
			printk("homing: brake 3.12 ms\n");
			ph = P_BRAKE;
			return (struct seg){ SEG_BRAKE, 0, R_BRAKE, 0 };
		case P_BRAKE:
		case P_DONE:
		default:
			ph = P_DONE;
			return (struct seg){ SEG_NONE, 0, 0, 0 };
		}
	}
}

static void pwm_handler(nrfx_pwm_event_type_t ev, void *ctx)
{
	ARG_UNUSED(ctx);
	if (ev == NRFX_PWM_EVENT_END_SEQ0 || ev == NRFX_PWM_EVENT_END_SEQ1) {
		int n = ev == NRFX_PWM_EVENT_END_SEQ0 ? 0 : 1;

		if (kind_of[n] == SEG_BRAKE) {
			/* The brake's last value has loaded: stop at the end of its period. */
			(void)nrfx_pwm_stop(&pwm, false);
			return;
		}
		struct seg s = next_seg();

		if (s.kind != SEG_NONE) {
			fill(n, &s);
			nrfx_pwm_sequence_update(&pwm, (uint8_t)n, &seq[n]);
		} else {
			kind_of[n] = SEG_NONE;
		}
	} else if (ev == NRFX_PWM_EVENT_STOPPED) {
		k_sem_give(&stopped);
	}
}

/* d x 640 for current i_ua: MD-3, with r in uOhm and VSYS in uV. */
static uint16_t amplitude(int64_t i_ua, int64_t r_uohm, int64_t vsys_uv)
{
	int64_t num = (int64_t)TOP * i_ua * r_uohm;
	int64_t den = vsys_uv * 1000000LL;
	int64_t a = (num + den / 2) / den;

	return (uint16_t)(a > TOP ? TOP : (a < 0 ? 0 : a));
}

int main(void)
{
	int16_t sample;
	struct adc_sequence aseq = {
		.buffer = &sample,
		.buffer_size = sizeof(sample),
	};
	nrfx_pwm_config_t cfg = {
		.output_pins = { PIN_A_IN1, PIN_A_IN2, PIN_B_IN1, PIN_B_IN2 },
		.pin_inverted = { false, false, false, false },
		.irq_priority = 1,
		.base_clock = NRF_PWM_CLK_16MHz,
		.count_mode = NRF_PWM_MODE_UP,
		.top_value = TOP,
		.load_mode = NRF_PWM_LOAD_INDIVIDUAL,
		.step_mode = NRF_PWM_STEP_AUTO,
		.skip_gpio_cfg = false,
		.skip_psel_cfg = false,
	};

	if (!gpio_is_ready_dt(&nsleep) || !gpio_is_ready_dt(&home) || !adc_is_ready_dt(&vsense)) {
		printk("homing: device not ready\n");
		return 0;
	}
	(void)gpio_pin_configure_dt(&nsleep, GPIO_OUTPUT_INACTIVE);
	(void)gpio_pin_configure_dt(&home, GPIO_INPUT);  /* no pull: U42 is push-pull */

	/* The idle VSYS reading (drivers asleep), E2's channel. */
	if (adc_channel_setup_dt(&vsense) < 0 || adc_sequence_init_dt(&vsense, &aseq) < 0 ||
	    adc_read_dt(&vsense, &aseq) < 0) {
		printk("homing: adc failed\n");
		return 0;
	}
	int64_t code = sample < 0 ? 0 : sample;
	/* VSYS = code x 3.6 V / 4096 x full / output = code x 2.8125 mV: one LSB at the pin is
	 * 878 906 250 pV, the divider ratio is taken in units of 100 kOhm (32 / 10), as E2 does. */
	int64_t vsys_uv = code * 878906250LL * (FULL_OHMS / 100000) / (OUTPUT_OHMS / 100000) /
			  1000000LL;

	/* The die temperature (TEMP, 0.25 degC units; DATARDY 36 us after START). */
	nrf_temp_event_clear(NRF_TEMP, NRF_TEMP_EVENT_DATARDY);
	nrf_temp_task_trigger(NRF_TEMP, NRF_TEMP_TASK_START);
	while (!nrf_temp_event_check(NRF_TEMP, NRF_TEMP_EVENT_DATARDY)) {
	}
	int32_t tq = nrf_temp_result_get(NRF_TEMP);

	nrf_temp_event_clear(NRF_TEMP, NRF_TEMP_EVENT_DATARDY);
	nrf_temp_task_trigger(NRF_TEMP, NRF_TEMP_TASK_STOP);

	/* MD-3: R_eff = 20 Ohm x (1 + 0.00393 x max(T - 25 C, 0)) + 0.33 Ohm; 19.65 mOhm per 0.25 C. */
	int32_t dtq = tq > 100 ? tq - 100 : 0;
	int64_t r_uohm = R_COIL_UOHM + R_DRV_UOHM + (R_COIL_UOHM / 1000000LL) * 3930LL / 4 * dtq;

	amp_run = amplitude(I_RUN_UA, r_uohm, vsys_uv);
	amp_stall = amplitude(I_STALL_UA, r_uohm, vsys_uv);
	printk("homing: vsys=%d uV (code %d) temp=%d qC r_eff=%d uOhm d_run=%u/640 d_stall=%u/640\n",
	       (int)vsys_uv, (int)code, (int)tq, (int)r_uohm, amp_run, amp_stall);

	IRQ_CONNECT(PWM0_IRQn, 1, nrfx_pwm_irq_handler, &pwm, 0);
	if (nrfx_pwm_init(&pwm, &cfg, pwm_handler, NULL) != 0) {
		printk("homing: nrfx_pwm_init failed\n");
		return 0;
	}

	for (int attempt = 1; attempt <= ATTEMPTS; attempt++) {
		u = 0;  /* the stored phase: a first boot (state/carriage invalid) */
		failed = false;
		seen_released = false;
		enter(P_H0);
		printk("homing: H0 wake: nSLEEP high (attempt %d%s)\n", attempt,
		       slow ? ", every travel phase at 313 PPS" : "");
		(void)gpio_pin_set_dt(&nsleep, 1);
		k_sleep(K_MSEC(1));   /* tWAKE <= 30 us */
		struct seg s0 = next_seg();
		struct seg s1 = next_seg();

		fill(0, &s0);
		fill(1, &s1);
		(void)nrfx_pwm_complex_playback(&pwm, &seq[0], &seq[1], 1,
						NRFX_PWM_FLAG_LOOP | NRFX_PWM_FLAG_SIGNAL_END_SEQ0 |
						NRFX_PWM_FLAG_SIGNAL_END_SEQ1 |
						NRFX_PWM_FLAG_NO_EVT_FINISHED);
		(void)k_sem_take(&stopped, K_FOREVER);
		(void)gpio_pin_set_dt(&nsleep, 0);  /* the IN pins are low again: coast, then sleep */
		if (!failed) {
#ifdef CONFIG_HOMING_STEP_TEST
			printk("homing: step test done: u=%d, drivers asleep\n", (int)u);
#else
			printk("homing: IDLE: x = x_park (park stop - %d steps), u=%d, drivers asleep "
			       "(attempt %d)\n", BACKOFF_STEPS, (int)u, attempt);
#endif
			return 0;
		}
		slow = true;
	}
	printk("homing: FAULT (E_DET_HOME): %d failed attempts, drivers asleep\n", ATTEMPTS);
	return 0;
}
