/* nrf-zephyr gate image (ADR-008 D1, D3): an unmodified Zephyr kernel on
 * Breadboard's nRF52840 platform.
 *
 *   P0.13 high around k_busy_wait(10000)   (arch_busy_wait = nrfx busy-wait)
 *   P0.14 high around k_msleep(10)         (RTC1 system timer)
 *   P0.15 toggles every k_msleep(50), forever
 *
 * Pins are driven raw through the gpio0 controller, so the board's LED
 * polarity does not matter. Rebuilt by scripts/nrf_zephyr_gate.sh. */
#include <zephyr/kernel.h>
#include <zephyr/drivers/gpio.h>

static const struct device *const gpio0 = DEVICE_DT_GET(DT_NODELABEL(gpio0));

int main(void)
{
    for (gpio_pin_t p = 13; p <= 15; p++) {
        gpio_pin_configure(gpio0, p, GPIO_OUTPUT_LOW);
    }

    gpio_pin_set_raw(gpio0, 13, 1);
    k_busy_wait(10000);
    gpio_pin_set_raw(gpio0, 13, 0);

    gpio_pin_set_raw(gpio0, 14, 1);
    k_msleep(10);
    gpio_pin_set_raw(gpio0, 14, 0);

    for (;;) {
        k_msleep(50);
        gpio_pin_toggle(gpio0, 15);
    }
    return 0;
}
