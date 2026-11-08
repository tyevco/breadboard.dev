/*
 * Test firmware (ADR-003 step 2 gate): a busy loop that copies input PB0 to
 * output PA6, writing PA6 only when PB0 changes. The firmware sees an input
 * the kernel drives at time t at the lattice point ceil_L(t), never earlier,
 * so PA6 follows at ceil_L(t) + 1 fs. Used by
 * engines/cpu-renode/tests/blinky.rs.
 */
#include <stdint.h>

#define REG32(addr) (*(volatile uint32_t *)(addr))

#define RCC_APB2ENR REG32(0x40021018u)
#define RCC_APB2ENR_IOPAEN (1u << 2)
#define RCC_APB2ENR_IOPBEN (1u << 3)

#define GPIOA_CRL REG32(0x40010800u)
#define GPIOA_BSRR REG32(0x40010810u)
#define GPIOB_IDR REG32(0x40010C08u)

#define OUT_PIN 6u

int main(void)
{
    RCC_APB2ENR |= RCC_APB2ENR_IOPAEN | RCC_APB2ENR_IOPBEN;
    /* PB0 stays a floating input (reset state). PA6: output 2 MHz, push-pull. */
    uint32_t crl = GPIOA_CRL;
    crl &= ~(0xFu << (OUT_PIN * 4u));
    crl |= 0x2u << (OUT_PIN * 4u);
    GPIOA_CRL = crl;
    GPIOA_BSRR = 1u << (OUT_PIN + 16u);

    uint32_t last = 0u;
    for (;;) {
        uint32_t v = GPIOB_IDR & 1u;
        if (v != last) {
            last = v;
            GPIOA_BSRR = v ? (1u << OUT_PIN) : (1u << (OUT_PIN + 16u));
        }
    }
}
