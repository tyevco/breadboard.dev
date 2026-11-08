/*
 * Timer-triggered ADC conversions with an analog watchdog, bare-metal
 * STM32F072 (Cortex-M0), for ADR-003 Part C (D6, approver decision 4) on
 * Renode 1.17.0's platforms/cpus/stm32f072.repl, whose ADC is
 * Analog.STM32F0_ADC (an STM32_ADC_Common, i.e. an IADC that the External
 * Control ADC command can drive with SetValue).
 *
 * - ADC channel 1 (PA1, analog mode), 12-bit, right aligned, one channel.
 * - TIM3 counts at 1 MHz (PSC from the timer clock) and updates every
 *   PERIOD_US (ARR = PERIOD_US - 1, as RM0091 specifies; Renode 1.17.0's
 *   STM32_Timer updates every ARR ticks, so 999 us there). Every update
 *   raises TIM3_IRQn.
 * - Default build (adc.elf): the TIM3 update ISR starts one conversion
 *   (ADSTART, software trigger). The conversion instant therefore follows
 *   TIM3, which the firmware configures; on hardware this costs the ISR
 *   latency against a TRGO trigger.
 * - -DADC_HW_TRIGGER (adc_hwtrig.elf): the hardware way, EXTSEL = TRG3
 *   (TIM3_TRGO), EXTEN = rising, TIM3 MMS = update, ADSTART once, TIM3 at
 *   2 ms. Renode's STM32_ADC_Common ignores EXTSEL and fires its own
 *   trigger at the repl's externalEventFrequency (1 kHz) instead; this
 *   build exists so renode_adc_test.py can pin that fact (ADR-003
 *   Amendment 5).
 * - Analog watchdog 1 on channel 1 only (AWDSGL), window [AWD_LT, AWD_HT]
 *   = [1.0 V, 2.0 V] at vref 3.3 V. A conversion outside the window sets
 *   AWD; the ISR counts it and drives PA5 (ALARM) high. The next
 *   conversion inside the window drives PA5 low again.
 * - Everything the host checks is in `status` at 0x20000000 (linker
 *   script), plus the PA5 edges.
 */
#include <stdint.h>

#define REG(addr) (*(volatile uint32_t *)(addr))

/* RCC (RM0091 section 6.4) */
#define RCC_BASE 0x40021000u
#define RCC_AHBENR REG(RCC_BASE + 0x14u)
#define RCC_APB2ENR REG(RCC_BASE + 0x18u)
#define RCC_APB1ENR REG(RCC_BASE + 0x1Cu)
#define RCC_AHBENR_IOPAEN (1u << 17)
#define RCC_APB2ENR_ADCEN (1u << 9)
#define RCC_APB1ENR_TIM3EN (1u << 1)

/* GPIOA (RM0091 section 8.4) */
#define GPIOA_BASE 0x48000000u
#define GPIOA_MODER REG(GPIOA_BASE + 0x00u)
#define GPIOA_BSRR REG(GPIOA_BASE + 0x18u)
#define ALARM_PIN 5u
#define ADC_PIN 1u

/* TIM3 (RM0091 section 17.4) */
#define TIM3_BASE 0x40000400u
#define TIM3_CR1 REG(TIM3_BASE + 0x00u)
#define TIM3_CR2 REG(TIM3_BASE + 0x04u)
#define TIM3_DIER REG(TIM3_BASE + 0x0Cu)
#define TIM3_SR REG(TIM3_BASE + 0x10u)
#define TIM3_EGR REG(TIM3_BASE + 0x14u)
#define TIM3_PSC REG(TIM3_BASE + 0x28u)
#define TIM3_ARR REG(TIM3_BASE + 0x2Cu)
#define TIM_CR1_CEN (1u << 0)
#define TIM_CR1_URS (1u << 2)
#define TIM_CR2_MMS_UPDATE (2u << 4)
#define TIM_DIER_UIE (1u << 0)
#define TIM_SR_UIF (1u << 0)
#define TIM_EGR_UG (1u << 0)

/* ADC (RM0091 section 13.12) */
#define ADC_BASE 0x40012400u
#define ADC_ISR REG(ADC_BASE + 0x00u)
#define ADC_IER REG(ADC_BASE + 0x04u)
#define ADC_CR REG(ADC_BASE + 0x08u)
#define ADC_CFGR1 REG(ADC_BASE + 0x0Cu)
#define ADC_SMPR REG(ADC_BASE + 0x14u)
#define ADC_TR REG(ADC_BASE + 0x20u)
#define ADC_CHSELR REG(ADC_BASE + 0x28u)
#define ADC_DR REG(ADC_BASE + 0x40u)
#define ADC_ISR_ADRDY (1u << 0)
#define ADC_ISR_EOSMP (1u << 1)
#define ADC_ISR_EOC (1u << 2)
#define ADC_ISR_EOSEQ (1u << 3)
#define ADC_ISR_OVR (1u << 4)
#define ADC_ISR_AWD (1u << 7)
#define ADC_IER_EOCIE (1u << 2)
#define ADC_IER_AWDIE (1u << 7)
#define ADC_CR_ADEN (1u << 0)
#define ADC_CR_ADSTART (1u << 2)
#define ADC_CR_ADCAL (1u << 31)
#define ADC_CFGR1_EXTSEL_TRG3 (3u << 6) /* TIM3_TRGO on STM32F0x2 */
#define ADC_CFGR1_EXTEN_RISING (1u << 10)
#define ADC_CFGR1_AWDSGL (1u << 22)
#define ADC_CFGR1_AWDEN (1u << 23)
#define ADC_CFGR1_AWDCH(ch) ((uint32_t)(ch) << 26)

/* NVIC */
#define NVIC_ISER REG(0xE000E100u)
#define ADC1_COMP_IRQN 12u
#define TIM3_IRQN 16u

/*
 * Renode's stm32f0.repl clocks every STM32_Timer at a fixed 10 MHz
 * (`frequency: 10000000`), whatever RCC says. Real silicon after reset runs
 * TIM3 from the 8 MHz HSI; change this for hardware.
 */
#ifndef TIMER_CLOCK_HZ
#define TIMER_CLOCK_HZ 10000000u
#endif

#ifdef ADC_HW_TRIGGER
#define PERIOD_US 2000u
#else
#define PERIOD_US 1000u
#endif

/* Window of analog watchdog 1, 12-bit codes: round(v / 3.3 V * 4095). */
#define AWD_LT 1241u /* 1.0 V */
#define AWD_HT 2482u /* 2.0 V */

#define STATUS_MAGIC 0x42424144u /* "DABB" in memory: bb ADc */

struct status {
    uint32_t magic;       /* 0x00 STATUS_MAGIC once initialised */
    uint32_t conversions; /* 0x04 EOC count */
    uint32_t last_code;   /* 0x08 last ADC_DR */
    uint32_t awd_events;  /* 0x0C AWD flags seen */
    uint32_t alarm;       /* 0x10 PA5 level driven */
    uint32_t tim_updates; /* 0x14 TIM3 update interrupts */
    uint32_t awd_lt;      /* 0x18 window, as programmed */
    uint32_t awd_ht;      /* 0x1C */
    uint32_t period_us;   /* 0x20 TIM3 period */
    uint32_t hw_trigger;  /* 0x24 1 for the ADC_HW_TRIGGER build */
};

__attribute__((section(".status"), used)) volatile struct status status;

void TIM3_IRQHandler(void)
{
    TIM3_SR = ~TIM_SR_UIF; /* rc_w0 */
    status.tim_updates++;
#ifndef ADC_HW_TRIGGER
    ADC_CR = ADC_CR_ADSTART;
#endif
}

void ADC1_COMP_IRQHandler(void)
{
    uint32_t isr = ADC_ISR;
    if (isr & ADC_ISR_AWD) {
        ADC_ISR = ADC_ISR_AWD; /* rc_w1 */
        status.awd_events++;
        GPIOA_BSRR = 1u << ALARM_PIN;
        status.alarm = 1u;
    }
    if (isr & ADC_ISR_EOC) {
        uint32_t code = ADC_DR; /* clears EOC */
        ADC_ISR = ADC_ISR_EOSMP | ADC_ISR_EOSEQ | ADC_ISR_OVR;
        status.last_code = code;
        status.conversions++;
        if (status.alarm && code >= AWD_LT && code <= AWD_HT) {
            GPIOA_BSRR = 1u << (ALARM_PIN + 16u);
            status.alarm = 0u;
        }
    }
}

int main(void)
{
    RCC_AHBENR |= RCC_AHBENR_IOPAEN;
    RCC_APB2ENR |= RCC_APB2ENR_ADCEN;
    RCC_APB1ENR |= RCC_APB1ENR_TIM3EN;

    /* PA1 analog (MODER = 11), PA5 general-purpose output (MODER = 01), low. */
    GPIOA_BSRR = 1u << (ALARM_PIN + 16u);
    GPIOA_MODER = (GPIOA_MODER & ~((3u << (2u * ADC_PIN)) | (3u << (2u * ALARM_PIN)))) |
                  (3u << (2u * ADC_PIN)) | (1u << (2u * ALARM_PIN));

    /* Calibrate with the ADC disabled, then enable it and wait for ADRDY. */
    ADC_CR = ADC_CR_ADCAL;
    while (ADC_CR & ADC_CR_ADCAL) {
    }
    ADC_CHSELR = 1u << ADC_PIN;
    ADC_SMPR = 0u;
    ADC_TR = (AWD_HT << 16) | AWD_LT;
    ADC_CFGR1 = ADC_CFGR1_AWDEN | ADC_CFGR1_AWDSGL | ADC_CFGR1_AWDCH(ADC_PIN)
#ifdef ADC_HW_TRIGGER
                | ADC_CFGR1_EXTSEL_TRG3 | ADC_CFGR1_EXTEN_RISING
#endif
        ;
    ADC_CR = ADC_CR_ADEN;
    while (!(ADC_ISR & ADC_ISR_ADRDY)) {
    }
    ADC_ISR = ADC_ISR_ADRDY;
    ADC_IER = ADC_IER_EOCIE | ADC_IER_AWDIE;

    status.awd_lt = AWD_LT;
    status.awd_ht = AWD_HT;
    status.period_us = PERIOD_US;
#ifdef ADC_HW_TRIGGER
    status.hw_trigger = 1u;
#endif

    /* TIM3: 1 MHz count, update every PERIOD_US, TRGO on update. */
    TIM3_PSC = TIMER_CLOCK_HZ / 1000000u - 1u;
    TIM3_ARR = PERIOD_US - 1u;
    TIM3_CR2 = TIM_CR2_MMS_UPDATE;
    TIM3_CR1 = TIM_CR1_URS;
    TIM3_EGR = TIM_EGR_UG; /* load PSC/ARR; URS: no interrupt for it */
    TIM3_SR = ~TIM_SR_UIF;
    TIM3_DIER = TIM_DIER_UIE;

    NVIC_ISER = (1u << ADC1_COMP_IRQN) | (1u << TIM3_IRQN);

#ifdef ADC_HW_TRIGGER
    ADC_CR = ADC_CR_ADSTART; /* arm: conversions now start on the trigger */
#endif
    status.magic = STATUS_MAGIC;
    TIM3_CR1 = TIM_CR1_URS | TIM_CR1_CEN;

    for (;;) {
        __asm volatile("wfi");
    }
}
