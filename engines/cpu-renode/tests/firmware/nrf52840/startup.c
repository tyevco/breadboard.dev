/* nRF52840 startup for Breadboard's bare-metal gate firmware (ADR-008 D5):
 * the vector table (the 16 Cortex-M4 exceptions and the nRF52840's 48
 * peripheral interrupts), .data copy, .bss clear, main. No SystemInit: it
 * only applies silicon errata workarounds, which Renode does not model.
 * Every handler not defined by a program is a weak alias of an endless
 * loop. */
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
WEAK(POWER_CLOCK_IRQHandler);
WEAK(GPIOTE_IRQHandler);
WEAK(TIMER0_IRQHandler);
WEAK(TIMER1_IRQHandler);
WEAK(TIMER2_IRQHandler);
WEAK(RTC0_IRQHandler);
WEAK(RTC1_IRQHandler);

/* nRF52840 PS v1.1, "Instantiation": peripheral ID = IRQ number. */
#define IRQ(n) (16 + (n))
__attribute__((section(".isr_vector"), used)) void (*const vector_table[16 + 48])(void) = {
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
    [IRQ(0)] = POWER_CLOCK_IRQHandler,
    [IRQ(6)] = GPIOTE_IRQHandler,
    [IRQ(8)] = TIMER0_IRQHandler,
    [IRQ(9)] = TIMER1_IRQHandler,
    [IRQ(10)] = TIMER2_IRQHandler,
    [IRQ(11)] = RTC0_IRQHandler,
    [IRQ(17)] = RTC1_IRQHandler,
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
