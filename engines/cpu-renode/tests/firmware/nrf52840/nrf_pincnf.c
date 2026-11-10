/* nrf-pincnf (ADR-008 D4): PIN_CNF configurations for the PIN_CNF audit.
 * The test binds the pins as noted and expects exactly these findings:
 *   P0.12 input                       bound digital_in            clean
 *   P0.14 output, S0S1                bound digital_out, S0S1     clean
 *   P0.13 output, H0H1                bound digital_out, S0S1     warn (DRIVE)
 *   P0.11 output                      bound digital_in            warn (DIR)
 *   P0.15 output, S0D1 (open drain)   bound digital_out           error
 *   P0.20 output                      unbound, net in scope       warn
 *   P0.21 input                       unbound, net in scope       clean
 *   P0.17 input at boot; an output (DIRSET) from the first RTC compare
 *         (~10 ms), an input again (PIN_CNF) from the second (~20 ms)
 *                                     bound digital_in            one mid-run
 *         warn at the first compare, clean at the end of the run
 * The boot configuration is written before the first WFI, so its mid-run
 * findings are stamped at time 0. RTC0 toggles P0.13 every 328 LFCLK ticks
 * so the run has edges. */
#include "nrf_gate.h"

#define PERIOD 328u

static void cnf(uint32_t n, uint32_t dir, uint32_t drive)
{
    NRF_P0->PIN_CNF[n] = (dir << GPIO_PIN_CNF_DIR_Pos)
                       | (GPIO_PIN_CNF_INPUT_Connect << GPIO_PIN_CNF_INPUT_Pos)
                       | (drive << GPIO_PIN_CNF_DRIVE_Pos);
}

static uint32_t compares;

void RTC0_IRQHandler(void)
{
    if (rtc0_rearm(PERIOD)) {
        pin_toggle(13);
        compares++;
        if (compares == 1) {
            NRF_P0->DIRSET = 1u << 17;
        } else if (compares == 2) {
            cnf(17, GPIO_PIN_CNF_DIR_Input, GPIO_PIN_CNF_DRIVE_S0S1);
        }
    }
}

int main(void)
{
    cnf(12, GPIO_PIN_CNF_DIR_Input, GPIO_PIN_CNF_DRIVE_S0S1);
    cnf(14, GPIO_PIN_CNF_DIR_Output, GPIO_PIN_CNF_DRIVE_S0S1);
    cnf(13, GPIO_PIN_CNF_DIR_Output, GPIO_PIN_CNF_DRIVE_H0H1);
    cnf(11, GPIO_PIN_CNF_DIR_Output, GPIO_PIN_CNF_DRIVE_S0S1);
    cnf(15, GPIO_PIN_CNF_DIR_Output, GPIO_PIN_CNF_DRIVE_S0D1);
    cnf(20, GPIO_PIN_CNF_DIR_Output, GPIO_PIN_CNF_DRIVE_S0S1);
    cnf(21, GPIO_PIN_CNF_DIR_Input, GPIO_PIN_CNF_DRIVE_S0S1);
    cnf(17, GPIO_PIN_CNF_DIR_Input, GPIO_PIN_CNF_DRIVE_S0S1);
    lfclk_start();
    rtc0_start(PERIOD);
    for (;;) {
        __WFI();
    }
}
