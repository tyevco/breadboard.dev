/* nrf-blinky (ADR-008 D3): RTC0 COMPARE0 toggles P0.13 every 1638 LFCLK
 * ticks (1638 / 32768 Hz = 49.987793 ms); the CPU sleeps in WFI between
 * compares. */
#include "nrf_gate.h"

#define LED 13u
#define PERIOD 1638u

void RTC0_IRQHandler(void)
{
    if (rtc0_rearm(PERIOD)) {
        pin_toggle(LED);
    }
}

int main(void)
{
    pin_output(LED);
    lfclk_start();
    rtc0_start(PERIOD);
    for (;;) {
        __WFI();
    }
}
