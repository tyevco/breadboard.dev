/*
 * Cortex-M0 startup for STM32F072: vector table (16 core exceptions plus the
 * 32 STM32F0 IRQ lines, RM0091 Table 37), .status/.data/.bss set-up, then
 * main(). Unused vectors fall into Default_Handler; device IRQs other than
 * ADC1_COMP (12) and TIM3 (16) are never enabled.
 */
#include <stdint.h>

extern uint32_t _sidata, _sdata, _edata, _sbss, _ebss, _sstatus, _estatus, _estack;

int main(void);

void Reset_Handler(void);
void Default_Handler(void);

void NMI_Handler(void) __attribute__((weak, alias("Default_Handler")));
void HardFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SVC_Handler(void) __attribute__((weak, alias("Default_Handler")));
void PendSV_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SysTick_Handler(void) __attribute__((weak, alias("Default_Handler")));
void ADC1_COMP_IRQHandler(void) __attribute__((weak, alias("Default_Handler")));
void TIM3_IRQHandler(void) __attribute__((weak, alias("Default_Handler")));

#define CORE_VECTORS 16
#define DEVICE_IRQS 32
#define IRQ(n) (CORE_VECTORS + (n))

__attribute__((section(".isr_vector"), used)) void (*const vector_table[CORE_VECTORS + DEVICE_IRQS])(void) = {
    [0] = (void (*)(void))(&_estack),
    [1] = Reset_Handler,
    [2] = NMI_Handler,
    [3] = HardFault_Handler,
    [11] = SVC_Handler,
    [14] = PendSV_Handler,
    [15] = SysTick_Handler,
    [IRQ(12)] = ADC1_COMP_IRQHandler, /* ADC and comparators */
    [IRQ(16)] = TIM3_IRQHandler,
};

static void zero(uint32_t *dst, uint32_t *end)
{
    while (dst < end) {
        *dst++ = 0u;
    }
}

void Reset_Handler(void)
{
    uint32_t *src = &_sidata;
    for (uint32_t *dst = &_sdata; dst < &_edata;) {
        *dst++ = *src++;
    }
    zero(&_sstatus, &_estatus);
    zero(&_sbss, &_ebss);
    main();
    for (;;) {
    }
}

void Default_Handler(void)
{
    for (;;) {
    }
}
