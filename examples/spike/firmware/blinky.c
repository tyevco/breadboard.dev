/*
 * Breadboard Phase 0 spike firmware: bare-metal blinky for STM32F103C8.
 *
 * PA5 is a push-pull output. The SysTick interrupt toggles it every 50 ms,
 * which gives a 10 Hz edge rate (a 5 Hz square wave).
 *
 * Clock: we do not touch RCC clock configuration. In Renode's
 * platforms/cpus/stm32f103.repl the NVIC (and therefore SysTick) is declared
 * with `systickFrequency: 72000000`, independent of what RCC is programmed to,
 * so SYSTICK_HZ below is 72 MHz. This was confirmed by experiment: PA5 edges
 * come out exactly 50 ms apart in Renode (see engines/cpu-renode/tests).
 * On real silicon running from the 8 MHz HSI reset clock, set SYSTICK_HZ to
 * 8000000 (or configure the PLL for 72 MHz first).
 */
#include <stdint.h>

#ifndef SYSTICK_HZ
#define SYSTICK_HZ 72000000u
#endif
#define TOGGLE_MS 50u

#define REG32(addr) (*(volatile uint32_t *)(addr))

/* RCC (RM0008 section 7.3) */
#define RCC_BASE 0x40021000u
#define RCC_APB2ENR REG32(RCC_BASE + 0x18u)
#define RCC_APB2ENR_IOPAEN (1u << 2)

/* GPIOA (RM0008 section 9.2) */
#define GPIOA_BASE 0x40010800u
#define GPIOA_CRL REG32(GPIOA_BASE + 0x00u)
#define GPIOA_ODR REG32(GPIOA_BASE + 0x0Cu)
#define GPIOA_BSRR REG32(GPIOA_BASE + 0x10u)

/* SysTick (ARMv7-M ARM B3.3) */
#define SYST_CSR REG32(0xE000E010u)
#define SYST_RVR REG32(0xE000E014u)
#define SYST_CVR REG32(0xE000E018u)
#define SYST_CSR_ENABLE (1u << 0)
#define SYST_CSR_TICKINT (1u << 1)
#define SYST_CSR_CLKSOURCE (1u << 2) /* processor clock */

#define LED_PIN 5u

void SysTick_Handler(void)
{
    if (GPIOA_ODR & (1u << LED_PIN)) {
        GPIOA_BSRR = 1u << (LED_PIN + 16u); /* reset */
    } else {
        GPIOA_BSRR = 1u << LED_PIN; /* set */
    }
}

int main(void)
{
    RCC_APB2ENR |= RCC_APB2ENR_IOPAEN;

    /* PA5: MODE5 = 0b10 (output, 2 MHz), CNF5 = 0b00 (general push-pull). */
    uint32_t crl = GPIOA_CRL;
    crl &= ~(0xFu << (LED_PIN * 4u));
    crl |= 0x2u << (LED_PIN * 4u);
    GPIOA_CRL = crl;
    GPIOA_BSRR = 1u << (LED_PIN + 16u); /* start low */

    SYST_RVR = (SYSTICK_HZ / 1000u) * TOGGLE_MS - 1u; /* 3 599 999 at 72 MHz: fits 24 bits */
    SYST_CVR = 0u;
    SYST_CSR = SYST_CSR_CLKSOURCE | SYST_CSR_TICKINT | SYST_CSR_ENABLE;

    for (;;) {
        __asm__ volatile("wfi");
    }
}
