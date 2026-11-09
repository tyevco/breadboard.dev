/*
 * Test firmware for ADR-005 (examples/adc/bb_adc_test.py), bare-metal
 * STM32F072:
 *
 * - -DTEST_IDLE (adc_idle.elf): wfi forever with nothing enabled. The
 *   register-level tests drive the ADC and the timers from the host
 *   (External Control SYSTEM_BUS) while Renode is paused.
 * - -DTEST_PWM (tim3_pwm.elf): TIM3 channel 1 in PWM mode 1 on PA6
 *   (alternate function 1), 1 MHz count, ARR = 99, CCR1 = 30, then wfi.
 *   On Breadboard's STM32F072 overlay the PWM reaches PA6 through the
 *   restated alternate-function wiring of bbTimer3; the test compares its
 *   PA6 edges with the stock platform's.
 */
#include <stdint.h>

#define REG(addr) (*(volatile uint32_t *)(addr))

#if defined(TEST_PWM)
#define RCC_BASE 0x40021000u
#define RCC_AHBENR REG(RCC_BASE + 0x14u)
#define RCC_APB1ENR REG(RCC_BASE + 0x1Cu)
#define GPIOA_BASE 0x48000000u
#define GPIOA_MODER REG(GPIOA_BASE + 0x00u)
#define GPIOA_AFRL REG(GPIOA_BASE + 0x20u)
#define TIM3_BASE 0x40000400u
#define TIM3_CR1 REG(TIM3_BASE + 0x00u)
#define TIM3_EGR REG(TIM3_BASE + 0x14u)
#define TIM3_CCMR1 REG(TIM3_BASE + 0x18u)
#define TIM3_CCER REG(TIM3_BASE + 0x20u)
#define TIM3_PSC REG(TIM3_BASE + 0x28u)
#define TIM3_ARR REG(TIM3_BASE + 0x2Cu)
#define TIM3_CCR1 REG(TIM3_BASE + 0x34u)
#endif

int main(void)
{
#if defined(TEST_PWM)
    RCC_AHBENR |= 1u << 17;  /* IOPAEN */
    RCC_APB1ENR |= 1u << 1;  /* TIM3EN */
    GPIOA_MODER = (GPIOA_MODER & ~(3u << 12)) | (2u << 12); /* PA6 AF */
    GPIOA_AFRL = (GPIOA_AFRL & ~(0xFu << 24)) | (1u << 24); /* AF1: TIM3_CH1 */
    TIM3_PSC = 9u;                                          /* 10 MHz / 10 */
    TIM3_ARR = 99u;
    TIM3_CCR1 = 30u;
    TIM3_CCMR1 = 6u << 4; /* OC1M = 110, PWM mode 1 */
    TIM3_CCER = 1u;       /* CC1E */
    TIM3_EGR = 1u;
    TIM3_CR1 = 1u; /* CEN */
#elif !defined(TEST_IDLE)
#error "define TEST_IDLE or TEST_PWM"
#endif
    for (;;) {
        __asm volatile("wfi");
    }
}
