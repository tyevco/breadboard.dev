/*
 * Breadboard feedback example, polling variant (ADR-003 plan step 8) for
 * STM32F103C8.
 *
 * The same board and pins as feedback.c, but the CPU never sleeps:
 *
 * - SysTick toggles PA5 every 50 ms (as feedback.c).
 * - The main loop busy-polls PB0 and copies it to PA6. There is no EXTI
 *   interrupt and no `wfi`.
 *
 * A CPU that never sleeps never promises to stay quiet, so lookahead
 * (ADR-003 D5) gains nothing on this board: every grant stays one cell and
 * the only cost of `--lookahead on` is the backed-off Queries (D5.5). The
 * step 8 gate requires its outputs to equal the `--lookahead off` run and
 * the simulation to be at most 10 % slower.
 *
 * Renode stamps a GPIO write with the sync point that opens its slice
 * (ADR-003 Context fact 2), so PA6 follows a PB0 change applied at
 * ceil_L(t_x) at ceil_L(t_x) + 1 fs, as with the EXTI firmware.
 *
 * SysTick runs at 72 MHz in Renode's stm32f103.repl (see blinky.c).
 */
#include <stdint.h>

#define SYSTICK_HZ 72000000u
#define TOGGLE_MS 50u

#define REG32(addr) (*(volatile uint32_t *)(addr))

/* RCC (RM0008 7.3) */
#define RCC_APB2ENR REG32(0x40021018u)
#define RCC_APB2ENR_IOPAEN (1u << 2)
#define RCC_APB2ENR_IOPBEN (1u << 3)

/* GPIO (RM0008 9.2) */
#define GPIOA_CRL REG32(0x40010800u)
#define GPIOA_ODR REG32(0x4001080Cu)
#define GPIOA_BSRR REG32(0x40010810u)
#define GPIOB_IDR REG32(0x40010C08u)

/* SysTick (ARMv7-M B3.3) */
#define SYST_CSR REG32(0xE000E010u)
#define SYST_RVR REG32(0xE000E014u)
#define SYST_CVR REG32(0xE000E018u)

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

int main(void)
{
    RCC_APB2ENR |= RCC_APB2ENR_IOPAEN | RCC_APB2ENR_IOPBEN;

    /* PA5, PA6: output 2 MHz push-pull (MODE = 0b10, CNF = 0b00), low. PB0
     * stays a floating input (reset state). */
    uint32_t crl = GPIOA_CRL;
    crl &= ~((0xFu << (DRIVE_PIN * 4u)) | (0xFu << (LED_PIN * 4u)));
    crl |= (0x2u << (DRIVE_PIN * 4u)) | (0x2u << (LED_PIN * 4u));
    GPIOA_CRL = crl;
    GPIOA_BSRR = (1u << (DRIVE_PIN + 16u)) | (1u << (LED_PIN + 16u));

    SYST_RVR = (SYSTICK_HZ / 1000u) * TOGGLE_MS - 1u;
    SYST_CVR = 0u;
    SYST_CSR = (1u << 2) | (1u << 1) | (1u << 0); /* processor clock, TICKINT, ENABLE */

    /* Copy PB0 to PA6 on a change only, so PA6 has one edge per PB0
     * transition. */
    uint32_t last = 0u;
    for (;;) {
        uint32_t in = GPIOB_IDR & 1u;
        if (in != last) {
            GPIOA_BSRR = in ? (1u << LED_PIN) : (1u << (LED_PIN + 16u));
            last = in;
        }
    }
}
