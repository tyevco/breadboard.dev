/* nrf-feedback (ADR-008 D3), the nRF52840 analogue of
 * examples/feedback/firmware/feedback.c: RTC0 toggles P0.13 every 1638
 * LFCLK ticks (49.987793 ms); GPIOTE channel 0 (EVENT mode, P0.11, TOGGLE)
 * interrupts on both edges of P0.11, and its ISR copies P0.11 to P0.15. The
 * CPU sleeps in WFI. Board: examples/nrf-feedback/board.cir. */
#include "nrf_gate.h"

#define PIN_OUT 13u
#define PIN_IN 11u
#define PIN_COPY 15u
#define PERIOD 1638u

void RTC0_IRQHandler(void)
{
    if (rtc0_rearm(PERIOD)) {
        pin_toggle(PIN_OUT);
    }
}

void GPIOTE_IRQHandler(void)
{
    if (NRF_GPIOTE->EVENTS_IN[0]) {
        NRF_GPIOTE->EVENTS_IN[0] = 0;
        if (NRF_P0->IN & (1u << PIN_IN)) {
            pin_set(PIN_COPY);
        } else {
            pin_clear(PIN_COPY);
        }
    }
    if (NRF_GPIOTE->EVENTS_PORT) {
        NRF_GPIOTE->EVENTS_PORT = 0;
    }
}

int main(void)
{
    pin_output(PIN_OUT);
    pin_output(PIN_COPY);
    pin_input(PIN_IN);
    NRF_GPIOTE->CONFIG[0] = (GPIOTE_CONFIG_MODE_Event << GPIOTE_CONFIG_MODE_Pos)
                          | (PIN_IN << GPIOTE_CONFIG_PSEL_Pos)
                          | (GPIOTE_CONFIG_POLARITY_Toggle << GPIOTE_CONFIG_POLARITY_Pos);
    NRF_GPIOTE->INTENSET = GPIOTE_INTENSET_IN0_Msk;
    NVIC_EnableIRQ(GPIOTE_IRQn);
    lfclk_start();
    rtc0_start(PERIOD);
    for (;;) {
        __WFI();
    }
}
