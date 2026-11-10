/* Shared helpers of the nRF52840 gate firmware (ADR-008 D3, D5). */
#ifndef NRF_GATE_H
#define NRF_GATE_H

#include <stdint.h>

#include "nrf52840.h"
#include "nrf52840_bitfields.h"

/* P0.n as a push-pull output (standard drive S0S1), driven low. */
static inline void pin_output(uint32_t n)
{
    NRF_P0->OUTCLR = 1u << n;
    NRF_P0->PIN_CNF[n] = GPIO_PIN_CNF_DIR_Output << GPIO_PIN_CNF_DIR_Pos;
}

/* P0.n as a connected input, no pull. */
static inline void pin_input(uint32_t n)
{
    NRF_P0->PIN_CNF[n] = (GPIO_PIN_CNF_DIR_Input << GPIO_PIN_CNF_DIR_Pos)
                       | (GPIO_PIN_CNF_INPUT_Connect << GPIO_PIN_CNF_INPUT_Pos);
}

static inline void pin_set(uint32_t n) { NRF_P0->OUTSET = 1u << n; }
static inline void pin_clear(uint32_t n) { NRF_P0->OUTCLR = 1u << n; }

static inline void pin_toggle(uint32_t n)
{
    if (NRF_P0->OUT & (1u << n)) {
        NRF_P0->OUTCLR = 1u << n;
    } else {
        NRF_P0->OUTSET = 1u << n;
    }
}

/* Start the 32.768 kHz LFCLK (RC source) and wait for LFCLKSTARTED. */
static inline void lfclk_start(void)
{
    NRF_CLOCK->LFCLKSRC = CLOCK_LFCLKSRC_SRC_RC << CLOCK_LFCLKSRC_SRC_Pos;
    NRF_CLOCK->EVENTS_LFCLKSTARTED = 0;
    NRF_CLOCK->TASKS_LFCLKSTART = 1;
    while (!NRF_CLOCK->EVENTS_LFCLKSTARTED) {
    }
}

/* RTC0 COMPARE0 interrupt every `ticks` (RTC0_IRQHandler re-arms it). */
static inline void rtc0_start(uint32_t ticks)
{
    NRF_RTC0->PRESCALER = 0;
    NRF_RTC0->CC[0] = ticks;
    NRF_RTC0->INTENSET = RTC_INTENSET_COMPARE0_Msk;
    NVIC_EnableIRQ(RTC0_IRQn);
    NRF_RTC0->TASKS_START = 1;
}

/* Clear COMPARE0 and move CC[0] on by `ticks`; true if it had fired. */
static inline int rtc0_rearm(uint32_t ticks)
{
    if (!NRF_RTC0->EVENTS_COMPARE[0]) {
        return 0;
    }
    NRF_RTC0->EVENTS_COMPARE[0] = 0;
    NRF_RTC0->CC[0] = (NRF_RTC0->CC[0] + ticks) & RTC_CC_COMPARE_Msk;
    return 1;
}

#endif
