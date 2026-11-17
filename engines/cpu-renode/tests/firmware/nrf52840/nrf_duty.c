/* nrf-duty (ADR-009 D4 and gate G6, Phase 2 milestone P3): a known WFI duty
 * for the hand calculation of the supply_load's average current.
 *
 * At boot: HFXO started (TASKS_HFCLKSTART, never stopped: #hfxo is high for
 * the whole run), the LFCLK, and RTC0 COMPARE0 every PERIOD = 32 LFCLK
 * ticks (976.5625 us). Each compare wakes the CPU (the ISR clears the event
 * and moves CC[0] on by PERIOD); the main loop then stays awake in a delay
 * loop of SPIN iterations of two instructions (subs, bne: one inline asm
 * block) and sleeps in WFI again. At the platform's 64 MIPS (nrf52840-bb@3,
 * 15.625 ns per instruction) the loop takes 2 * 7812 * 15.625 ns =
 * 244.125 us, so the CPU runs 244.125 us of every 976.5625 us: a duty of
 * 0.249984, plus the ISR and the loop's few instructions around it (well
 * under 1 us). The delay is counted in instructions, not by polling the RTC:
 * Renode's RTC COUNTER, read by a running CPU, advances at its sync points
 * (one lattice cell, measured: a poll for 8 ticks overshot by 9.4 us). No
 * GPIO is used. Board: examples/nrf-idd (bench/pi.toml, bench/pi-radio.toml). */
#include "nrf_gate.h"

#define PERIOD 32u
#define SPIN 7812u

void RTC0_IRQHandler(void)
{
    (void)rtc0_rearm(PERIOD);
}

int main(void)
{
    NRF_CLOCK->TASKS_HFCLKSTART = 1;
    lfclk_start();
    rtc0_start(PERIOD);
    for (;;) {
        __WFI();
        uint32_t n = SPIN;
        __asm volatile("1: subs %0, #1\n"
                       "   bne 1b\n"
                       : "+r"(n)
                       :
                       : "cc");
    }
}
