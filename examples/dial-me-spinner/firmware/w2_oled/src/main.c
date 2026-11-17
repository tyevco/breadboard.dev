/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * W2 firmware (Phase 2 plan section 2 W2 and milestone W-fw; ADR-009 D7): the S4 brown-out
 * reboot loop of dial-me-spinner docs/firmware.md (at 6854d25), USB at ILIM 100 mA with a dead
 * cell. Two images from one source:
 *
 *   w2_old_rule.elf (CONFIG_W2_OLD_RULE): the uncapped boot. W2_START_DELAY_MS after main()
 *          starts, OLED_VCC_EN (P1.02) rises (section 7.2 step 22: U21 TPS22917 connects VSYS to
 *          OLED_VIN and the TPS61040 boost starts, its surge collapsing VSYS). +3V3 falls below
 *          VBOR, the CPU resets, and the same boot enables the OLED again: the loop of the
 *          suite's strict xfail test_u21_turn_on_dead_battery_usb_100ma.
 *   w2_rule.elf: the NEW-1 rule (firmware.md section 6.3 "VBUS present but input limit still
 *          100 mA" and section 7.2): OLED_VCC_EN stays low unless the battery can supplement SYS
 *          (no BUVLO_STAT and VBAT_est >= 3.21 V from a battery-only reading in this power-up).
 *          Neither can be known here: Phase 2 has no TWIM bus device, so the firmware reads no
 *          STAT1 and has no battery-only reading (plan section 2 W1's recorded deviation). The
 *          supplement is therefore unknown and the OLED stays off; the LEDs show only the
 *          charging cue: LED_PWR_EN (P1.04) high at the same instant, and P0.06 (the cue's first
 *          frame, as in W1 and E1) W2_DIN_DELAY_MS later. The suite's passing
 *          ..._with_fw_rule[ILIM100-SYSREG4.5] case.
 *
 * Every boot reads RESETREAS, clears the bits it read (as the product's boot does, section 15)
 * and prints them with the rule on the console (UARTE0 TX on P1.00, the tap of ADR-008 D8): a
 * brown-out leaves no reset reason, so W2's old-rule check expects 0 after every reset (ADR-009
 * D7). The firmware keeps nothing in RAM across a reset (D7 keeps RAM; nothing here relies on it).
 *
 * POFCON is not used: W2 is about the OLED rule and the brown-out reset. In the suite's deck the
 * old rule's +3V3 crosses VBOR 3.83-4.09 ms after OLED_VCC_EN, inside firmware.md's 10 ms POFWARN
 * blanking of that edge (section 12.4), so a POF handler could not act before the reset; the
 * rule's +3V3 stays at 3.22 V or above. W1 is the POF case.
 */
#include <zephyr/kernel.h>
#include <zephyr/drivers/gpio.h>
#include <zephyr/sys/printk.h>
#include <hal/nrf_power.h>

static const struct gpio_dt_spec led_pwr_en =
	GPIO_DT_SPEC_GET(DT_NODELABEL(board_signals), led_pwr_en_gpios);
static const struct gpio_dt_spec led_din =
	GPIO_DT_SPEC_GET(DT_NODELABEL(board_signals), led_din_gpios);
static const struct gpio_dt_spec oled_vcc_en =
	GPIO_DT_SPEC_GET(DT_NODELABEL(board_signals), oled_vcc_en_gpios);

int main(void)
{
	uint32_t resetreas = nrf_power_resetreas_get(NRF_POWER);

	nrf_power_resetreas_clear(NRF_POWER, resetreas);
	if (!gpio_is_ready_dt(&led_pwr_en) || !gpio_is_ready_dt(&led_din) ||
	    !gpio_is_ready_dt(&oled_vcc_en)) {
		return 0;
	}
	(void)gpio_pin_configure_dt(&led_pwr_en, GPIO_OUTPUT_INACTIVE);
	(void)gpio_pin_configure_dt(&led_din, GPIO_OUTPUT_INACTIVE);
	(void)gpio_pin_configure_dt(&oled_vcc_en, GPIO_OUTPUT_INACTIVE);
	printk("w2: boot resetreas=0x%08x rule=%s\n", (unsigned int)resetreas,
	       IS_ENABLED(CONFIG_W2_OLD_RULE) ? "old" : "new");

	k_msleep(CONFIG_W2_START_DELAY_MS);

#ifdef CONFIG_W2_OLD_RULE
	/* The uncapped boot: the OLED boost on a 100 mA input with a dead cell. */
	(void)gpio_pin_set_dt(&oled_vcc_en, 1);
#else
	/* NEW-1: the supplement is unknown (no STAT1, no battery-only reading), so the OLED stays
	 * off and the LEDs show only the charging cue.
	 */
	(void)gpio_pin_set_dt(&led_pwr_en, 1);
	k_msleep(CONFIG_W2_DIN_DELAY_MS);
	(void)gpio_pin_set_dt(&led_din, 1);
#endif
	for (;;) {
		k_sleep(K_FOREVER);
	}
	return 0;
}
