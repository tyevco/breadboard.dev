/*
 * Sub-cell pulse gate firmware (ADR-003 approver decision 2, Amendment 12).
 *
 * EXTI0 <- PB0 (AFIO EXTICR1), rising edge only, unmasked. Each EXTI0
 * interrupt toggles PA6 and counts itself in `exti_count`. The main loop
 * is `wfi`. A High-then-Low pulse on PB0 shorter than one lattice cell
 * reaches Renode as two SET_STATEs at the same lattice point; the edge
 * detector latches the rising edge, so the ISR runs once and PA6 toggles
 * once, although PB0 is already low again when the CPU runs.
 */
#include "stm32f103.h"

/* AFIO (RM0008 9.4): EXTICR1 bits 3:0 select the port of EXTI0 (1 = PB). */
#define AFIO_EXTICR1 REG32(0x40010008u)

/* EXTI (RM0008 10.3) */
#define EXTI_IMR REG32(0x40010400u)
#define EXTI_RTSR REG32(0x40010408u)
#define EXTI_FTSR REG32(0x4001040Cu)
#define EXTI_PR REG32(0x40010414u)
#define EXTI0_IRQN 6u

volatile uint32_t exti_count;

void EXTI0_IRQHandler(void)
{
    EXTI_PR = 1u; /* clear the pending bit (write 1) */
    exti_count++;
    gpioa_toggle(6u);
}

int main(void)
{
    RCC_APB2ENR |= (1u << 0) | (1u << 2) | (1u << 3); /* AFIO, IOPA, IOPB */
    gpioa_output(6u); /* PB0 stays a floating input (reset state) */
    AFIO_EXTICR1 = (AFIO_EXTICR1 & ~0xFu) | 0x1u;
    EXTI_RTSR |= 1u;
    EXTI_FTSR &= ~1u;
    EXTI_PR = 1u;
    EXTI_IMR |= 1u;
    NVIC_ISER0 = 1u << EXTI0_IRQN;
    for (;;) {
        __asm__ volatile("wfi");
    }
}
