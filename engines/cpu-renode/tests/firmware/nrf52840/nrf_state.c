/* nrf-state (ADR-009 D4, Phase 2 milestone P3): the CPU state outputs
 * #run, #hfxo, #nvmc and #saadc at instants that can be derived by hand.
 *
 * Setup (C): P0.13 an output; the SAADC on AIN0 (TACQ 20 us, 12 bits, no
 * oversampling, RESULT.MAXCNT 1, the END interrupt enabled in the SAADC and
 * the NVIC); RTC0 COMPARE0 every 33 LFCLK ticks (about 1 ms), enabled in the
 * NVIC; then an erase-mode ERASEPAGE of the page at 0xF0000 (#nvmc high for
 * the NVMC model's tERASEPAGE, 85 ms, while the loop below runs), PRIMASK
 * set and the loop. With PRIMASK set a pending interrupt wakes WFI but no
 * handler runs, so the code between two WFIs is a fixed straight line.
 *
 * The loop (one inline asm block, so its instruction count is the text's):
 *
 *   A: wfi                     ; asleep until RTC0 COMPARE0 is pending
 *      str  r0, [r4, #0x140]   ; RTC0 EVENTS_COMPARE[0] = 0
 *      str  r2, [r5]           ; NVIC ICPR[0]: clear RTC0 and SAADC pending
 *      ldr  r3, [r4, #0x540]   ; RTC0 CC[0]
 *      adds r3, #33
 *      str  r3, [r4, #0x540]   ; CC[0] += 33
 *      str  r1, [r7]           ; CLOCK TASKS_HFCLKSTART = 1      (#hfxo rises)
 *      str  r1, [r6]           ; SAADC TASKS_START = 1          (#saadc rises)
 *      str  r1, [r6, #4]       ; SAADC TASKS_SAMPLE = 1         (#smp edge: a)
 *   B: wfi                     ; asleep until SAADC END (a + TACQ + tCONV)
 *      str  r0, [r6, #0x104]   ; SAADC EVENTS_END = 0
 *      str  r0, [r6, #0x100]   ; SAADC EVENTS_STARTED = 0
 *      str  r2, [r5]           ; NVIC ICPR[0]
 *      str  r1, [r6, #8]       ; SAADC TASKS_STOP = 1
 *      str  r1, [r7, #4]       ; CLOCK TASKS_HFCLKSTOP = 1       (#hfxo falls)
 *      b    A
 *
 * So the CPU wakes at A's RTC compare, executes 9 instructions (the 8 after
 * A and B's wfi) and sleeps from B; wakes at the SAADC's END, executes 7
 * (the 6 after B, the branch and A's wfi) and sleeps from A. The test
 * (engines/cpu-renode/tests/nrf52840.rs, nrf_state_outputs_exact) checks
 * #run against those counts at 64 MIPS (15.625 ns per instruction). */
#include "nrf_gate.h"

#define PIN_MARK 13u
#define PERIOD 33u
#define PAGE 0xF0000u

static volatile int16_t result;

int main(void)
{
    pin_output(PIN_MARK);
    NRF_SAADC->RESOLUTION = SAADC_RESOLUTION_VAL_12bit;
    NRF_SAADC->OVERSAMPLE = SAADC_OVERSAMPLE_OVERSAMPLE_Bypass;
    NRF_SAADC->CH[0].CONFIG = (SAADC_CH_CONFIG_GAIN_Gain1_6 << SAADC_CH_CONFIG_GAIN_Pos)
                            | (SAADC_CH_CONFIG_REFSEL_Internal << SAADC_CH_CONFIG_REFSEL_Pos)
                            | (SAADC_CH_CONFIG_TACQ_20us << SAADC_CH_CONFIG_TACQ_Pos);
    NRF_SAADC->CH[0].PSELP = SAADC_CH_PSELP_PSELP_AnalogInput0;
    NRF_SAADC->INTENSET = SAADC_INTENSET_END_Msk;
    NRF_SAADC->ENABLE = SAADC_ENABLE_ENABLE_Enabled;
    NRF_SAADC->RESULT.PTR = (uint32_t)&result;
    NRF_SAADC->RESULT.MAXCNT = 1;
    NVIC_EnableIRQ(SAADC_IRQn);

    lfclk_start();
    rtc0_start(PERIOD);

    NRF_NVMC->CONFIG = NVMC_CONFIG_WEN_Een << NVMC_CONFIG_WEN_Pos;
    NRF_NVMC->ERASEPAGE = PAGE;
    pin_set(PIN_MARK);
    __disable_irq();

    register uint32_t r0 __asm("r0") = 0;
    register uint32_t r1 __asm("r1") = 1;
    register uint32_t r2 __asm("r2") = (1u << RTC0_IRQn) | (1u << SAADC_IRQn);
    register uint32_t r4 __asm("r4") = (uint32_t)NRF_RTC0;
    register uint32_t r5 __asm("r5") = (uint32_t)&NVIC->ICPR[0];
    register uint32_t r6 __asm("r6") = (uint32_t)NRF_SAADC;
    register uint32_t r7 __asm("r7") = (uint32_t)&NRF_CLOCK->TASKS_HFCLKSTART;
    __asm volatile(
        "1:  wfi\n"
        "    str  r0, [r4, #0x140]\n"
        "    str  r2, [r5]\n"
        "    ldr  r3, [r4, #0x540]\n"
        "    adds r3, #33\n"
        "    str  r3, [r4, #0x540]\n"
        "    str  r1, [r7]\n"
        "    str  r1, [r6]\n"
        "    str  r1, [r6, #4]\n"
        "    wfi\n"
        "    str  r0, [r6, #0x104]\n"
        "    str  r0, [r6, #0x100]\n"
        "    str  r2, [r5]\n"
        "    str  r1, [r6, #8]\n"
        "    str  r1, [r7, #4]\n"
        "    b    1b\n"
        :
        : "r"(r0), "r"(r1), "r"(r2), "r"(r4), "r"(r5), "r"(r6), "r"(r7)
        : "r3", "memory");
    for (;;) {
    }
}
