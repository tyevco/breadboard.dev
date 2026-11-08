# Bare-metal STM32F103C8 firmware for examples/feedback and
# examples/feedback-open (ADR-003 plan step 4):
#   build/feedback.elf  SysTick toggles PA5 every 50 ms; the EXTI0 ISR (PB0,
#                       both edges) copies PB0 to PA6; main loop wfi
# Same flags and linker script as examples/spike/firmware. Deterministic
# build: no timestamps or absolute paths end up in the ELF.

CC      := arm-none-eabi-gcc
SIZE    := arm-none-eabi-size
BUILD   := build
LDSCRIPT := ../../spike/firmware/stm32f103c8.ld

CFLAGS  := -mcpu=cortex-m3 -mthumb -Os -std=c11 -Wall -Wextra -Werror \
           -ffreestanding -ffunction-sections -fdata-sections \
           -ffile-prefix-map=$(CURDIR)=.
LDFLAGS := -nostdlib -nostartfiles -T $(LDSCRIPT) -Wl,--gc-sections \
           -Wl,--build-id=none

.PHONY: all clean
.SECONDARY:

all: $(BUILD)/feedback.elf

$(BUILD)/%.o: %.c | $(BUILD)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/feedback.elf: $(BUILD)/startup.o $(BUILD)/feedback.o $(LDSCRIPT)
	$(CC) $(CFLAGS) $(LDFLAGS) -Wl,-Map=$(BUILD)/feedback.map $(BUILD)/startup.o $(BUILD)/feedback.o -o $@
	$(SIZE) $@

$(BUILD):
	mkdir -p $@

clean:
	rm -rf $(BUILD)
