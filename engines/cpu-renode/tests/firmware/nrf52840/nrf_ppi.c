/* nrf-ppi (ADR-008 D3, Context 2): TIMER1 at 1 MHz; PPI channel 0 routes
 * TIMER1 EVENTS_COMPARE[0] to GPIOTE TASKS_OUT[0] (TASK mode, P0.14,
 * TOGGLE), so P0.14 toggles in hardware at every compare. The TIMER1 ISR
 * (COMPARE0) moves CC[0] on by 1000 (Renode 1.17.0's TIMER ignores SHORTS
 * and never wraps, so the period is kept by the CPU) and toggles P0.16 from
 * the CPU. The CPU sleeps in WFI. Stock PPI defers the GPIOTE task to the
 * next synced state, so P0.14 lags the compare by up to one lattice cell;
 * the gate pins that latency and its determinism. */
#include "nrf_gate.h"

#define PPI_PIN 14u
#define CPU_PIN 16u
#define PERIOD_US 1000u

void TIMER1_IRQHandler(void)
{
    if (NRF_TIMER1->EVENTS_COMPARE[0]) {
        NRF_TIMER1->EVENTS_COMPARE[0] = 0;
        NRF_TIMER1->CC[0] += PERIOD_US;
        pin_toggle(CPU_PIN);
    }
}

int main(void)
{
    pin_output(CPU_PIN);
    NRF_GPIOTE->CONFIG[0] = (GPIOTE_CONFIG_MODE_Task << GPIOTE_CONFIG_MODE_Pos)
                          | (PPI_PIN << GPIOTE_CONFIG_PSEL_Pos)
                          | (GPIOTE_CONFIG_POLARITY_Toggle << GPIOTE_CONFIG_POLARITY_Pos)
                          | (GPIOTE_CONFIG_OUTINIT_Low << GPIOTE_CONFIG_OUTINIT_Pos);
    NRF_TIMER1->MODE = TIMER_MODE_MODE_Timer;
    NRF_TIMER1->BITMODE = TIMER_BITMODE_BITMODE_32Bit;
    NRF_TIMER1->PRESCALER = 4; /* 16 MHz / 2^4 = 1 MHz */
    NRF_TIMER1->CC[0] = PERIOD_US;
    NRF_TIMER1->INTENSET = TIMER_INTENSET_COMPARE0_Msk;
    NVIC_EnableIRQ(TIMER1_IRQn);
    NRF_PPI->CH[0].EEP = (uint32_t)&NRF_TIMER1->EVENTS_COMPARE[0];
    NRF_PPI->CH[0].TEP = (uint32_t)&NRF_GPIOTE->TASKS_OUT[0];
    NRF_PPI->CHENSET = PPI_CHENSET_CH0_Msk;
    NRF_TIMER1->TASKS_START = 1;
    for (;;) {
        __WFI();
    }
}
