/* nrf-saadc-model register tests (ADR-008 D7, Phase 1 M8): the CPU sleeps
 * in WFI with no interrupt enabled, so the host drives the SAADC through
 * External Control SYSTEM_BUS accesses alone (ADR-005 test plan 2's
 * adc_idle.elf). RAM at 0x20010000 is the tests' EasyDMA buffer. */
#include "nrf_gate.h"

int main(void)
{
    for (;;) {
        __WFI();
    }
}
