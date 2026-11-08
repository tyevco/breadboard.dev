/*
 * Lookahead gate firmware: SLEEPONEXIT (ADR-003 plan step 5).
 *
 * SCB_SCR.SLEEPONEXIT is set and main executes a single WFI. From then on
 * the CPU runs only the SysTick ISR (PA5 toggles every 5 ms) and goes back
 * to sleep on exception return, without executing main's WFI again. If
 * sleep-on-exit were not modelled, main would continue past its WFI and
 * toggle PA6 from a busy loop, which the gate would see as PA6 edges.
 */
#include "stm32f103.h"

void SysTick_Handler(void)
{
    gpioa_toggle(5u);
}

int main(void)
{
    RCC_APB2ENR |= 1u << 2; /* IOPA */
    gpioa_output(5u);
    gpioa_output(6u);
    SCB_SCR |= SCB_SCR_SLEEPONEXIT;
    systick_start_us(5000u);
    __asm__ volatile("wfi");
    for (;;) {
        gpioa_toggle(6u);
        for (volatile uint32_t i = 0u; i < 20000u; i++) {
        }
    }
}
