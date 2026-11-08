/*
 * Test firmware (ADR-003 step 2 gate): toggles PA5 from a busy delay loop,
 * never sleeping. Renode stamps a GPIO change with the sync point that opens
 * the slice in which the write executed (GPIOPort.cs), so the edges land on
 * lattice points (+1 fs, the lattice rule) and a coarser Renode quantum
 * gives coarser edges. Used by engines/cpu-renode/tests/blinky.rs.
 */
#include <stdint.h>

#define REG32(addr) (*(volatile uint32_t *)(addr))

#define RCC_APB2ENR REG32(0x40021018u)
#define RCC_APB2ENR_IOPAEN (1u << 2)

#define GPIOA_CRL REG32(0x40010800u)
#define GPIOA_ODR REG32(0x4001080Cu)
#define GPIOA_BSRR REG32(0x40010810u)

#define LED_PIN 5u
#ifndef DELAY_LOOPS
#define DELAY_LOOPS 21500u /* about 1.505 ms per toggle: not a multiple of 100 us */
#endif

int main(void)
{
    RCC_APB2ENR |= RCC_APB2ENR_IOPAEN;
    uint32_t crl = GPIOA_CRL;
    crl &= ~(0xFu << (LED_PIN * 4u));
    crl |= 0x2u << (LED_PIN * 4u); /* output 2 MHz, push-pull */
    GPIOA_CRL = crl;
    GPIOA_BSRR = 1u << (LED_PIN + 16u);

    for (;;) {
        for (volatile uint32_t i = 0; i < DELAY_LOOPS; i++) {
        }
        if (GPIOA_ODR & (1u << LED_PIN)) {
            GPIOA_BSRR = 1u << (LED_PIN + 16u);
        } else {
            GPIOA_BSRR = 1u << LED_PIN;
        }
    }
}
