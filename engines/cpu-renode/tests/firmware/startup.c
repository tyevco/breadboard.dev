/*
 * Cortex-M3 startup for the STM32F103 lookahead gate firmware (ADR-003
 * plan step 5): vector table with the 16 core exceptions and the 60
 * STM32F103 IRQ lines, .data copy, .bss zeroing, then main(). Every handler
 * is a weak alias of Default_Handler, so each firmware defines only the
 * ones it enables.
 */
#include <stdint.h>

extern uint32_t _sidata, _sdata, _edata, _sbss, _ebss, _estack;

int main(void);

void Reset_Handler(void);
void Default_Handler(void);

#define WEAK(name) void name(void) __attribute__((weak, alias("Default_Handler")))
WEAK(NMI_Handler);
WEAK(HardFault_Handler);
WEAK(MemManage_Handler);
WEAK(BusFault_Handler);
WEAK(UsageFault_Handler);
WEAK(SVC_Handler);
WEAK(DebugMon_Handler);
WEAK(PendSV_Handler);
WEAK(SysTick_Handler);
WEAK(EXTI0_IRQHandler);
WEAK(DMA1_Channel2_IRQHandler);
WEAK(TIM1_UP_IRQHandler);
WEAK(TIM1_CC_IRQHandler);
WEAK(TIM2_IRQHandler);
WEAK(USART1_IRQHandler);

#define CORE_VECTORS 16
#define DEVICE_IRQS 60
#define IRQ(n) (CORE_VECTORS + (n))

__attribute__((section(".isr_vector"), used)) void (*const vector_table[CORE_VECTORS + DEVICE_IRQS])(void) = {
    [0] = (void (*)(void))(&_estack),
    [1] = Reset_Handler,
    [2] = NMI_Handler,
    [3] = HardFault_Handler,
    [4] = MemManage_Handler,
    [5] = BusFault_Handler,
    [6] = UsageFault_Handler,
    [11] = SVC_Handler,
    [12] = DebugMon_Handler,
    [14] = PendSV_Handler,
    [15] = SysTick_Handler,
    /* RM0008 Table 63 */
    [IRQ(6)] = EXTI0_IRQHandler,
    [IRQ(12)] = DMA1_Channel2_IRQHandler,
    [IRQ(25)] = TIM1_UP_IRQHandler,
    [IRQ(27)] = TIM1_CC_IRQHandler,
    [IRQ(28)] = TIM2_IRQHandler,
    [IRQ(37)] = USART1_IRQHandler,
};

void Reset_Handler(void)
{
    uint32_t *src = &_sidata;
    for (uint32_t *dst = &_sdata; dst < &_edata;) {
        *dst++ = *src++;
    }
    for (uint32_t *dst = &_sbss; dst < &_ebss;) {
        *dst++ = 0u;
    }
    main();
    for (;;) {
    }
}

void Default_Handler(void)
{
    for (;;) {
    }
}
