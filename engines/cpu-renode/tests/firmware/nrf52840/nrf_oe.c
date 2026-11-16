/* nrf-oe (ADR-009 D5, Phase 2 milestone P2): direction writes next to GPIO
 * writes, for the #oe outputs (PIN_CNF.DIR from the engine's PIN_CNF write
 * hook). Each RTC0 compare (every 328 LFCLK ticks) alternates:
 *   odd:  DIRSET P0.17, then OUTSET P0.14 | P0.17  (adjacent stores)
 *   even: OUTCLR P0.14 | P0.17, then PIN_CNF[17] = input (adjacent stores)
 * so in program order P0.17's #oe rises right before P0.14 (and P0.17) rise,
 * and falls right after they fall. P0.14 is configured at boot; P0.17 is
 * an input (#oe low) until the first compare. */
#include "nrf_gate.h"

#define PERIOD 328u

static uint32_t compares;

void RTC0_IRQHandler(void)
{
    if (rtc0_rearm(PERIOD)) {
        compares++;
        if (compares & 1u) {
            NRF_P0->DIRSET = 1u << 17;
            NRF_P0->OUTSET = (1u << 14) | (1u << 17);
        } else {
            NRF_P0->OUTCLR = (1u << 14) | (1u << 17);
            NRF_P0->PIN_CNF[17] = (GPIO_PIN_CNF_DIR_Input << GPIO_PIN_CNF_DIR_Pos)
                                | (GPIO_PIN_CNF_INPUT_Disconnect << GPIO_PIN_CNF_INPUT_Pos);
        }
    }
}

int main(void)
{
    pin_output(14);
    lfclk_start();
    rtc0_start(PERIOD);
    for (;;) {
        __WFI();
    }
}
