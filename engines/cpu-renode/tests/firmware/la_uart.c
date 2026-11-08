/*
 * Lookahead gate firmware: interrupt-driven UART TX (ADR-003 plan step 5).
 *
 * Every 5 ms the SysTick ISR starts sending "bb\r\n" on USART1 by enabling
 * the TXE interrupt; the USART1 ISR writes the next character and, after
 * the last one, disables TXEIE and toggles PA5. No host backend is
 * attached to the UART (the lookahead self-test requires that). Between
 * messages the CPU sleeps in WFI.
 */
#include "stm32f103.h"

static const char msg[] = "bb\r\n";
static volatile unsigned next;

void SysTick_Handler(void)
{
    next = 0u;
    USART1_CR1 |= USART_CR1_TXEIE;
}

void USART1_IRQHandler(void)
{
    if (!(USART1_SR & USART_SR_TXE)) {
        return;
    }
    if (next < sizeof msg - 1u) {
        USART1_DR = (uint32_t)(unsigned char)msg[next++];
        return;
    }
    USART1_CR1 &= ~USART_CR1_TXEIE;
    gpioa_toggle(5u);
}

int main(void)
{
    RCC_APB2ENR |= (1u << 2) | (1u << 14); /* IOPA, USART1 */
    gpioa_output(5u);
    USART1_BRR = 72000000u / 115200u;
    USART1_CR1 = USART_CR1_UE | USART_CR1_TE;
    NVIC_ISER1 = 1u << (37u - 32u);
    systick_start_us(5000u);
    for (;;) {
        __asm__ volatile("wfi");
    }
}
