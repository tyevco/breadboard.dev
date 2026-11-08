/*
 * The few STM32F103 / Cortex-M3 registers the lookahead gate firmware uses
 * (RM0008, ARMv7-M ARM). SysTick runs at 72 MHz in Renode's stm32f103.repl;
 * the general-purpose timers count at 10 MHz before the prescaler.
 */
#ifndef BB_LA_STM32F103_H
#define BB_LA_STM32F103_H

#include <stdint.h>

#define REG32(addr) (*(volatile uint32_t *)(addr))

#define SYSTICK_HZ 72000000u
#define TIMER_HZ 10000000u

/* RCC (RM0008 7.3) */
#define RCC_AHBENR REG32(0x40021014u)
#define RCC_APB2ENR REG32(0x40021018u)
#define RCC_APB1ENR REG32(0x4002101Cu)

/* AFIO (RM0008 9.4) */
#define AFIO_MAPR REG32(0x40010004u)

/* GPIO (RM0008 9.2) */
#define GPIOA_CRL REG32(0x40010800u)
#define GPIOA_CRH REG32(0x40010804u)
#define GPIOA_ODR REG32(0x4001080Cu)
#define GPIOA_BSRR REG32(0x40010810u)
#define GPIOA_BSRR_ADDR 0x40010810u

/* TIM1 (RM0008 14.4) and TIM2 (15.4) */
#define TIM1_BASE 0x40012C00u
#define TIM2_BASE 0x40000000u
#define TIM_CR1(b) REG32((b) + 0x00u)
#define TIM_DIER(b) REG32((b) + 0x0Cu)
#define TIM_SR(b) REG32((b) + 0x10u)
#define TIM_EGR(b) REG32((b) + 0x14u)
#define TIM_CCMR1(b) REG32((b) + 0x18u)
#define TIM_CCER(b) REG32((b) + 0x20u)
#define TIM_PSC(b) REG32((b) + 0x28u)
#define TIM_ARR(b) REG32((b) + 0x2Cu)
#define TIM_CCR1(b) REG32((b) + 0x34u)
#define TIM_BDTR(b) REG32((b) + 0x44u)

/* DMA1 channel 2 (RM0008 13.4; Renode's STM32G0DMA register layout) */
#define DMA1_IFCR REG32(0x40020004u)
#define DMA1_CCR2 REG32(0x4002001Cu)
#define DMA1_CNDTR2 REG32(0x40020020u)
#define DMA1_CPAR2 REG32(0x40020024u)
#define DMA1_CMAR2 REG32(0x40020028u)

/* USART1 (RM0008 27.6) */
#define USART1_SR REG32(0x40013800u)
#define USART1_DR REG32(0x40013804u)
#define USART1_BRR REG32(0x40013808u)
#define USART1_CR1 REG32(0x4001380Cu)
#define USART_SR_TXE (1u << 7)
#define USART_CR1_UE (1u << 13)
#define USART_CR1_TXEIE (1u << 7)
#define USART_CR1_TE (1u << 3)

/* Cortex-M3 core */
#define SYST_CSR REG32(0xE000E010u)
#define SYST_RVR REG32(0xE000E014u)
#define SYST_CVR REG32(0xE000E018u)
#define NVIC_ISER0 REG32(0xE000E100u)
#define NVIC_ISER1 REG32(0xE000E104u)
#define SCB_SCR REG32(0xE000ED10u)
#define SCB_SCR_SLEEPONEXIT (1u << 1)

/* The Breadboard lookahead block (only the engine may use it). */
#define BB_LOOKAHEAD_ADDR 0x5FFF0000u

/* PAn as a 2 MHz push-pull output (MODE = 0b10, CNF = 0b00), driven low. */
static inline void gpioa_output(unsigned pin)
{
    volatile uint32_t *cr = pin < 8u ? &GPIOA_CRL : &GPIOA_CRH;
    unsigned shift = (pin % 8u) * 4u;
    *cr = (*cr & ~(0xFu << shift)) | (0x2u << shift);
    GPIOA_BSRR = 1u << (pin + 16u);
}

static inline void gpioa_toggle(unsigned pin)
{
    if (GPIOA_ODR & (1u << pin)) {
        GPIOA_BSRR = 1u << (pin + 16u);
    } else {
        GPIOA_BSRR = 1u << pin;
    }
}

/* SysTick from the processor clock with its interrupt, every `us` us. */
static inline void systick_start_us(uint32_t us)
{
    SYST_RVR = (SYSTICK_HZ / 1000000u) * us - 1u;
    SYST_CVR = 0u;
    SYST_CSR = (1u << 2) | (1u << 1) | (1u << 0);
}

#endif
