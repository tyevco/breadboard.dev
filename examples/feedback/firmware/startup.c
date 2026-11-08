/*
 * Cortex-M3 startup for STM32F103 with device interrupts: vector table (16
 * core exceptions plus the 60 STM32F103 IRQ lines), .data copy, .bss
 * zeroing, then main(). Unused core vectors fall into Default_Handler;
 * device IRQs other than EXTI0 are never enabled and stay 0.
 *
 * The spike's examples/spike/firmware/startup.c has core exceptions only;
 * it is kept unchanged so blinky.elf (and the spike outputs) stay
 * byte-identical.
 */
#include <stdint.h>

extern uint32_t _sidata, _sdata, _edata, _sbss, _ebss, _estack;

int main(void);

void Reset_Handler(void);
void Default_Handler(void);

void NMI_Handler(void) __attribute__((weak, alias("Default_Handler")));
void HardFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void MemManage_Handler(void) __attribute__((weak, alias("Default_Handler")));
void BusFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void UsageFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SVC_Handler(void) __attribute__((weak, alias("Default_Handler")));
void DebugMon_Handler(void) __attribute__((weak, alias("Default_Handler")));
void PendSV_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SysTick_Handler(void) __attribute__((weak, alias("Default_Handler")));
void EXTI0_IRQHandler(void) __attribute__((weak, alias("Default_Handler")));

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
    /* Device IRQs the firmware never enables stay 0. */
    [IRQ(6)] = EXTI0_IRQHandler, /* EXTI line 0 (RM0008 Table 63) */
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
