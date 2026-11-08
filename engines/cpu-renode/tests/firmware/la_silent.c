/*
 * Lookahead gate firmware: silent wakes (ADR-003 plan step 5).
 *
 * SysTick fires every 1 ms; its ISR only counts, and every 10th tick it
 * toggles PA5. The nine wakes in between produce no edge, so an over-long
 * quiet promise is visible only to the wake audit.
 *
 * Before sleeping, main reads the lookahead block's STATUS and writes to it,
 * as firmware must not: the block ignores both (a read returns 0) and
 * logs them. If the read returned anything else, PA6 would go high, which
 * the gate compares against a run without the block.
 */
#include "stm32f103.h"

static volatile uint32_t ticks;

void SysTick_Handler(void)
{
    if (++ticks % 10u == 0u) {
        gpioa_toggle(5u);
    }
}

int main(void)
{
    RCC_APB2ENR |= 1u << 2; /* IOPA */
    gpioa_output(5u);
    gpioa_output(6u);
    uint32_t status = REG32(BB_LOOKAHEAD_ADDR);
    REG32(BB_LOOKAHEAD_ADDR + 4u) = 0xFFFFFFFFu;
    if (status != 0u) {
        GPIOA_BSRR = 1u << 6;
    }
    systick_start_us(1000u);
    for (;;) {
        __asm__ volatile("wfi");
    }
}
