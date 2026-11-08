# STM32F103C8 gate firmware for the Renode lookahead plugin (ADR-003 plan
# step 5), used by engines/cpu-renode/tests/lookahead.rs:
#   build/la_tim1pwm.elf      TIM1 PWM on PA8 (timer-driven pin), SysTick PA5
#   build/la_dma.elf          TIM2 update -> DMA1 ch2 -> GPIOA_BSRR (PA5)
#   build/la_uart.elf         interrupt-driven USART1 TX, PA5 per message
#   build/la_sleeponexit.elf  SLEEPONEXIT: SysTick ISR only (PA5)
#   build/la_wfe.elf          WFE/SEV main loop (PA5 in the ISR, PA6 in main)
#   build/la_silent.elf       silent SysTick wakes, PA5 every 10th
# Same flags and linker script as examples/spike/firmware. Deterministic
# build: no timestamps or absolute paths end up in the ELFs.

CC      := arm-none-eabi-gcc
SIZE    := arm-none-eabi-size
BUILD   := build
LDSCRIPT := ../../../../examples/spike/firmware/stm32f103c8.ld
PROGS   := la_tim1pwm la_dma la_uart la_sleeponexit la_wfe la_silent
TARGETS := $(PROGS:%=$(BUILD)/%.elf)

CFLAGS  := -mcpu=cortex-m3 -mthumb -Os -std=c11 -Wall -Wextra -Werror \
           -ffreestanding -ffunction-sections -fdata-sections \
           -ffile-prefix-map=$(CURDIR)=.
LDFLAGS := -nostdlib -nostartfiles -T $(LDSCRIPT) -Wl,--gc-sections \
           -Wl,--build-id=none

.PHONY: all clean
.SECONDARY:

all: $(TARGETS)

$(BUILD)/%.o: %.c stm32f103.h | $(BUILD)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/%.elf: $(BUILD)/startup.o $(BUILD)/%.o $(LDSCRIPT)
	$(CC) $(CFLAGS) $(LDFLAGS) -Wl,-Map=$(BUILD)/$*.map $(BUILD)/startup.o $(BUILD)/$*.o -o $@
	$(SIZE) $@

$(BUILD):
	mkdir -p $@

clean:
	rm -rf $(BUILD)
