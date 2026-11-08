/*
 * Lookahead gate firmware: TIM1 PWM on PA8 (ADR-003 plan step 5).
 *
 * TIM1 channel 1 runs PWM mode 1 at 1 kHz, 25 % duty, on PA8 (alternate
 * function push-pull). The pin is driven by the timer model through the
 * platform's alternate-function connection, never by the CPU, which sleeps
 * in WFI. SysTick toggles PA5 every 5 ms. The PWM edges come from TIM1's
 * compare and update events, which are clock limits, so a quiet promise
 * must end at each of them.
 */
#include "stm32f103.h"

void SysTick_Handler(void)
{
    gpioa_toggle(5u);
}

int main(void)
{
    RCC_APB2ENR |= (1u << 0) | (1u << 2) | (1u << 11); /* AFIO, IOPA, TIM1 */
    gpioa_output(5u);
    /* PA8: alternate function push-pull, 2 MHz (MODE = 0b10, CNF = 0b10). */
    GPIOA_CRH = (GPIOA_CRH & ~0xFu) | 0xAu;
    /* TIM1_REMAP = 0b00 (CH1 on PA8). Renode's STM32F1AFIO routes a timer
     * to its pins on a write of the remap field, so write it explicitly. */
    AFIO_MAPR = AFIO_MAPR & ~(0x3u << 6);

    TIM_PSC(TIM1_BASE) = TIMER_HZ / 1000000u - 1u; /* 1 MHz */
    TIM_ARR(TIM1_BASE) = 1000u - 1u;               /* 1 kHz */
    TIM_CCR1(TIM1_BASE) = 250u;                    /* 25 % */
    TIM_CCMR1(TIM1_BASE) = (0x6u << 4) | (1u << 3); /* OC1M = PWM 1, OC1PE */
    TIM_CCER(TIM1_BASE) = 1u;                      /* CC1E */
    TIM_BDTR(TIM1_BASE) = 1u << 15;                /* MOE */
    TIM_EGR(TIM1_BASE) = 1u;                       /* UG: load the shadows */
    TIM_CR1(TIM1_BASE) = 1u;                       /* CEN */

    systick_start_us(5000u);
    for (;;) {
        __asm__ volatile("wfi");
    }
}
