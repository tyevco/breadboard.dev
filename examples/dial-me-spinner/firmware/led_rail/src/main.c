/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * E1 firmware (ADR-008 D5, Phase 1 plan section 2 E1, M6): the LED rail sequencing rule of
 * dial-me-spinner docs/firmware.md section 6.4 (at 6854d25), written with the Zephyr APIs that
 * section names, on the board subset in ../boards/dial/dial_me_spinner:
 *
 *   On:  regulator_enable(vled)        LED_PWR_EN (P1.04) high, then the regulator framework
 *                                      sleeps (k_sleep) before it returns;
 *        P0.06 high                    stands in for the I2S's first frame (the product hands
 *                                      P0.06 back to the I2S here; Renode's I2S drives no pin).
 *   Off: P0.06 low                     gpio_pin_configure_dt(&led_din, GPIO_OUTPUT_LOW);
 *        regulator_disable(vled)       LED_PWR_EN low;
 *        k_msleep(LED_OFF_MS)          at least 30 ms before the next turn-on (QOD, LSW-2).
 *
 * Every rule wait is a k_sleep on the RTC system timer, never a busy-wait (ADR-008 D1: busy
 * loops run at about 2/3 of their requested time on the 64 MIPS platform).
 *
 * Zephyr v4.5.0's regulator framework (drivers/regulator/regulator_common.c) sleeps for
 * off-on-delay-us (30 ms) after it enables a regulator, and applies startup-delay-us (10 ms) only
 * to a regulator enabled at boot. So P0.06 rises 30 ms after LED_PWR_EN, not the 10 ms
 * firmware.md section 6.4 expects from startup-delay-us; the rule (at least 10 ms) holds either
 * way, and the minimum off time before a turn-on is the firmware's own k_msleep below.
 *
 * CONFIG_LED_RAIL_WRONG_ORDER (led_rail_wrong_order.elf) swaps the off order: LED_PWR_EN low
 * first and P0.06 low LED_RAIL_WRONG_DIN_MS later, so P0.06 is high while VLED is off (LSW-3).
 *
 * The cycle repeats forever: 5 ms of "frames", then off for LED_OFF_MS.
 */
#include <zephyr/kernel.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/drivers/regulator.h>

#define LED_FRAMES_MS 5
#define LED_OFF_MS    35 /* >= 30 ms (firmware.md section 6.4); the suite's sequence uses 35 */

static const struct device *const vled = DEVICE_DT_GET(DT_NODELABEL(vled));
static const struct gpio_dt_spec led_din =
	GPIO_DT_SPEC_GET(DT_NODELABEL(board_signals), led_din_gpios);

static void rail_on(void)
{
	(void)regulator_enable(vled);
	(void)gpio_pin_set_dt(&led_din, 1);
}

static void rail_off(void)
{
#ifdef CONFIG_LED_RAIL_WRONG_ORDER
	(void)regulator_disable(vled);
	k_msleep(CONFIG_LED_RAIL_WRONG_DIN_MS);
	(void)gpio_pin_configure_dt(&led_din, GPIO_OUTPUT_LOW);
#else
	(void)gpio_pin_configure_dt(&led_din, GPIO_OUTPUT_LOW);
	(void)regulator_disable(vled);
#endif
}

int main(void)
{
	if (!device_is_ready(vled) || !gpio_is_ready_dt(&led_din)) {
		return 0;
	}
	/* DIN never floats on R41 alone: an output driven low from the start. */
	(void)gpio_pin_configure_dt(&led_din, GPIO_OUTPUT_LOW);
	for (;;) {
		rail_on();
		k_msleep(LED_FRAMES_MS);
		rail_off();
		k_msleep(LED_OFF_MS);
	}
	return 0;
}
