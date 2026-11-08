/*
 * Lookahead gate firmware: DMA -> GPIOA_BSRR (ADR-003 plan step 5).
 *
 * TIM2 updates every 1 ms. Each update raises TIM2's update request line,
 * which the platform connects to DMA1 channel 2; the channel copies the
 * next word of a two-entry table (set PA5, reset PA5) to GPIOA_BSRR, in
 * circular mode, so PA5 toggles every 1 ms without the CPU writing it.
 *
 * Renode's STM32_Timer raises the update request only while the update
 * interrupt is enabled and pending (UIF & UIE), so the TIM2 ISR must clear
 * UIF for the next request; it does nothing else (a silent wake). The
 * channel uses the peripheral-to-memory direction with the table as the
 * "peripheral" address (incremented) and BSRR as the "memory" address:
 * Renode's model copies one unit per request in that direction, but the
 * whole block at once from memory to peripheral.
 */
#include "stm32f103.h"

static const uint32_t table[2] = {1u << 5, 1u << (5u + 16u)};

void TIM2_IRQHandler(void)
{
    TIM_SR(TIM2_BASE) = 0u; /* clear UIF: re-arms the DMA request line */
}

int main(void)
{
    RCC_AHBENR |= 1u << 0;  /* DMA1 */
    RCC_APB2ENR |= 1u << 2; /* IOPA */
    RCC_APB1ENR |= 1u << 0; /* TIM2 */
    gpioa_output(5u);

    DMA1_CPAR2 = (uint32_t)&table[0];
    DMA1_CMAR2 = GPIOA_BSRR_ADDR;
    DMA1_CNDTR2 = 2u;
    /* DIR = 0, CIRC, PINC, PSIZE = MSIZE = 32 bit, EN */
    DMA1_CCR2 = (1u << 5) | (1u << 6) | (0x2u << 8) | (0x2u << 10) | 1u;

    TIM_PSC(TIM2_BASE) = TIMER_HZ / 1000000u - 1u; /* 1 MHz */
    TIM_ARR(TIM2_BASE) = 1000u - 1u;               /* 1 ms */
    TIM_DIER(TIM2_BASE) = 1u;                      /* UIE (also the DMA request) */
    NVIC_ISER0 = 1u << 28;
    TIM_CR1(TIM2_BASE) = 1u;

    for (;;) {
        __asm__ volatile("wfi");
    }
}
