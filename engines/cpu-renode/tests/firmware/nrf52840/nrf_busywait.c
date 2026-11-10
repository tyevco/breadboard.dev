/* nrf-busywait (ADR-008 D1): the virtual time of a 10 ms nrfx busy-wait and
 * of a 10 ms RTC sleep.
 *
 * 1. P0.13 high, nrfx_coredep_delay_us(10000), P0.13 low. The delay is
 *    nrfx's own machine code (modules/hal/nordic/nrfx/lib/nrfx_coredep.h,
 *    hal_nordic c708b9fe, the non-DWT ARM path that Zephyr's
 *    arch_busy_wait uses on nRF52): `SUBS r0, #3; BHI .-2; BX LR` run with
 *    r0 = time_us * 64, i.e. 2 instructions per 3 assumed cycles. Renode
 *    counts instructions at PerformanceInMips, so at 64 MIPS the wait lasts
 *    about 2/3 of the requested time (ADR-008 Context 2).
 * 2. P0.14 high, sleep in WFI until RTC0 COMPARE0 328 LFCLK ticks later
 *    (328 / 32768 Hz = 10.009766 ms, the tick count a 10 ms sleep rounds up
 *    to), P0.14 low. Timer-based, so exact to within one RTC tick.
 * Then WFI forever. */
#include "nrf_gate.h"

#define BUSY_PIN 13u
#define SLEEP_PIN 14u
#define SLEEP_TICKS 328u
#define BUSY_US 10000u
#define CPU_MHZ 64u
#define LOOP_CYCLES 3u /* NRFX_COREDEP_DELAY_US_LOOP_CYCLES */

__attribute__((aligned(16))) static const uint16_t delay_machine_code[] = {
    0x3800 + LOOP_CYCLES, /* SUBS r0, #loop_cycles */
    0xd8fd,               /* BHI .-2 */
    0x4770                /* BX LR */
};

static void nrfx_coredep_delay_us(uint32_t time_us)
{
    typedef void (*delay_func_t)(uint32_t);
    const delay_func_t delay_cycles = (delay_func_t)((((uint32_t)delay_machine_code) | 1));
    delay_cycles(time_us * CPU_MHZ);
}

static volatile uint32_t woke;

void RTC0_IRQHandler(void)
{
    if (NRF_RTC0->EVENTS_COMPARE[0]) {
        NRF_RTC0->EVENTS_COMPARE[0] = 0;
        NRF_RTC0->INTENCLR = RTC_INTENCLR_COMPARE0_Msk;
        woke = 1;
    }
}

int main(void)
{
    pin_output(BUSY_PIN);
    pin_output(SLEEP_PIN);
    lfclk_start();

    pin_set(BUSY_PIN);
    nrfx_coredep_delay_us(BUSY_US);
    pin_clear(BUSY_PIN);

    NRF_RTC0->PRESCALER = 0;
    NRF_RTC0->TASKS_START = 1;
    pin_set(SLEEP_PIN);
    NRF_RTC0->CC[0] = (NRF_RTC0->COUNTER + SLEEP_TICKS) & RTC_CC_COMPARE_Msk;
    NRF_RTC0->INTENSET = RTC_INTENSET_COMPARE0_Msk;
    NVIC_EnableIRQ(RTC0_IRQn);
    while (!woke) {
        __WFI();
    }
    pin_clear(SLEEP_PIN);
    for (;;) {
        __WFI();
    }
}
