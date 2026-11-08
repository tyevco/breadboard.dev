/*
 * Lookahead gate firmware: WFE/SEV (ADR-003 plan step 5).
 *
 * The main loop sleeps with WFE instead of WFI. The SysTick ISR (every
 * 5 ms) toggles PA5 and sets a flag; the exception return is a wake-up
 * event, so main's WFE returns, and main toggles PA6. Before the loop main
 * runs SEV then WFE, which must return at once (the event register is set)
 * and consume the event.
 */
#include "stm32f103.h"

static volatile uint32_t ticks;

void SysTick_Handler(void)
{
    gpioa_toggle(5u);
    ticks++;
}

int main(void)
{
    RCC_APB2ENR |= 1u << 2; /* IOPA */
    gpioa_output(5u);
    gpioa_output(6u);
    __asm__ volatile("sev");
    __asm__ volatile("wfe");
    systick_start_us(5000u);
    uint32_t seen = 0u;
    for (;;) {
        __asm__ volatile("wfe");
        if (ticks != seen) {
            seen = ticks;
            gpioa_toggle(6u);
        }
    }
}
