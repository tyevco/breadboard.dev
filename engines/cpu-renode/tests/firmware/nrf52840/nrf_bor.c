/* nrf-bor (Phase 2 P5, ADR-009 D7): what a brown-out reset leaves behind.
 * Every boot counts itself in a RAM word that no startup code touches
 * (BOOTS, above .data/.bss and below the stack), reads RESETREAS, clears the
 * bits it read and drives:
 *   P0.14  high when RESETREAS was not 0 at this boot
 *   P0.15  high from the second boot on (the RAM word survived the reset)
 *   P0.16  high when GPREGRET was not 0 at this boot; every boot then writes
 *          GPREGRET = 0x5A (a retained register a brown-out clears)
 *   P0.13  toggled by RTC0 every 33 LFCLK ticks (about 1 ms), after the boot
 *          writes; the CPU sleeps in WFI between compares.
 * POWER's RESETREAS (0x400) and GPREGRET (0x51C) are BbNrf52Power's. */
#include "nrf_gate.h"

#define LED 13u
#define PERIOD 33u
#define BOOTS (*(volatile uint32_t *)0x20030000u)

void RTC0_IRQHandler(void)
{
    if (rtc0_rearm(PERIOD)) {
        pin_toggle(LED);
    }
}

int main(void)
{
    uint32_t boots = BOOTS + 1u;
    BOOTS = boots;
    uint32_t reas = NRF_POWER->RESETREAS;
    NRF_POWER->RESETREAS = reas;
    uint32_t gp = NRF_POWER->GPREGRET;
    NRF_POWER->GPREGRET = 0x5Au;
    pin_output(14);
    pin_output(15);
    pin_output(16);
    if (reas != 0u) {
        pin_set(14);
    }
    if (boots >= 2u) {
        pin_set(15);
    }
    if (gp != 0u) {
        pin_set(16);
    }
    pin_output(LED);
    lfclk_start();
    rtc0_start(PERIOD);
    for (;;) {
        __WFI();
    }
}
