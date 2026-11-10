/* nrf-timer-sleep (ADR-008 D6, Amendment 1): TIMER0 at 1 MHz, COMPARE0 at
 * 1000 counts toggles P0.13 once (1 ms), then the CPU sleeps in WFI with
 * nothing else running. Renode 1.17.0's nRF TIMER is 64-bit and never wraps
 * (ADR-008 Context 2), so after the compare its next clock limit is about
 * 2^64 timer ticks away and the lookahead promise is far beyond Time's
 * range: the engine must clamp it (MAX_PROMISE_NS) instead of panicking. */
#include "nrf_gate.h"

#define PIN 13u

void TIMER0_IRQHandler(void)
{
    if (NRF_TIMER0->EVENTS_COMPARE[0]) {
        NRF_TIMER0->EVENTS_COMPARE[0] = 0;
        pin_toggle(PIN);
    }
}

int main(void)
{
    pin_output(PIN);
    NRF_TIMER0->MODE = TIMER_MODE_MODE_Timer;
    NRF_TIMER0->BITMODE = TIMER_BITMODE_BITMODE_32Bit;
    NRF_TIMER0->PRESCALER = 4; /* 16 MHz / 2^4 = 1 MHz */
    NRF_TIMER0->CC[0] = 1000;
    NRF_TIMER0->INTENSET = TIMER_INTENSET_COMPARE0_Msk;
    NVIC_EnableIRQ(TIMER0_IRQn);
    NRF_TIMER0->TASKS_START = 1;
    for (;;) {
        __WFI();
    }
}
