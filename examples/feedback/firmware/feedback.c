/*
 * Breadboard feedback example firmware (ADR-003 "Feedback-path
 * correctness", plan step 4) for STM32F103C8.
 *
 * - SysTick toggles PA5 every 50 ms (as the spike's blinky). On the
 *   feedback board PA5 charges an RC whose node feeds PB0 back.
 * - EXTI0 (PB0 through AFIO, both edges) runs an ISR that copies PB0 to
 *   PA6, which drives an LED.
 * - The main loop is `wfi`.
 *
 * SysTick runs at 72 MHz in Renode's stm32f103.repl (see blinky.c).
 */
#include <stdint.h>

#define SYSTICK_HZ 72000000u
#define TOGGLE_MS 50u

#define REG32(addr) (*(volatile uint32_t *)(addr))

/* RCC (RM0008 7.3) */
#define RCC_APB2ENR REG32(0x40021018u)
#define RCC_APB2ENR_AFIOEN (1u << 0)
#define RCC_APB2ENR_IOPAEN (1u << 2)
#define RCC_APB2ENR_IOPBEN (1u << 3)

/* AFIO (RM0008 9.4): EXTICR1 bits 3:0 select the port of EXTI0 (1 = PB). */
#define AFIO_EXTICR1 REG32(0x40010008u)

/* EXTI (RM0008 10.3) */
#define EXTI_IMR REG32(0x40010400u)
#define EXTI_RTSR REG32(0x40010408u)
#define EXTI_FTSR REG32(0x4001040Cu)
#define EXTI_PR REG32(0x40010414u)

/* GPIO (RM0008 9.2) */
#define GPIOA_CRL REG32(0x40010800u)
#define GPIOA_ODR REG32(0x4001080Cu)
#define GPIOA_BSRR REG32(0x40010810u)
#define GPIOB_IDR REG32(0x40010C08u)

/* SysTick (ARMv7-M B3.3) and NVIC */
#define SYST_CSR REG32(0xE000E010u)
#define SYST_RVR REG32(0xE000E014u)
#define SYST_CVR REG32(0xE000E018u)
#define NVIC_ISER0 REG32(0xE000E100u)
#define EXTI0_IRQN 6u

#define DRIVE_PIN 5u /* PA5: drives the RC */
#define LED_PIN 6u   /* PA6: follows PB0 */

void SysTick_Handler(void)
{
    if (GPIOA_ODR & (1u << DRIVE_PIN)) {
        GPIOA_BSRR = 1u << (DRIVE_PIN + 16u);
    } else {
        GPIOA_BSRR = 1u << DRIVE_PIN;
    }
}

void EXTI0_IRQHandler(void)
{
    EXTI_PR = 1u; /* clear the pending bit (write 1) */
    if (GPIOB_IDR & 1u) {
        GPIOA_BSRR = 1u << LED_PIN;
    } else {
        GPIOA_BSRR = 1u << (LED_PIN + 16u);
    }
}

int main(void)
{
    RCC_APB2ENR |= RCC_APB2ENR_AFIOEN | RCC_APB2ENR_IOPAEN | RCC_APB2ENR_IOPBEN;

    /* PA5, PA6: output 2 MHz push-pull (MODE = 0b10, CNF = 0b00), low. PB0
     * stays a floating input (reset state). */
    uint32_t crl = GPIOA_CRL;
    crl &= ~((0xFu << (DRIVE_PIN * 4u)) | (0xFu << (LED_PIN * 4u)));
    crl |= (0x2u << (DRIVE_PIN * 4u)) | (0x2u << (LED_PIN * 4u));
    GPIOA_CRL = crl;
    GPIOA_BSRR = (1u << (DRIVE_PIN + 16u)) | (1u << (LED_PIN + 16u));

    /* EXTI0 <- PB0, both edges, unmasked; enable its NVIC line. */
    AFIO_EXTICR1 = (AFIO_EXTICR1 & ~0xFu) | 0x1u;
    EXTI_RTSR |= 1u;
    EXTI_FTSR |= 1u;
    EXTI_PR = 1u;
    EXTI_IMR |= 1u;
    NVIC_ISER0 = 1u << EXTI0_IRQN;

    SYST_RVR = (SYSTICK_HZ / 1000u) * TOGGLE_MS - 1u;
    SYST_CVR = 0u;
    SYST_CSR = (1u << 2) | (1u << 1) | (1u << 0); /* processor clock, TICKINT, ENABLE */

    for (;;) {
        __asm__ volatile("wfi");
    }
}
