# Bare-metal STM32F103C8 firmware for the spike and the Renode engine tests:
#   build/blinky.elf    PA5 toggled by SysTick every 50 ms (the spike)
#   build/busyloop.elf  PA5 toggled from a busy delay loop (ADR-003 step 2)
#   build/copy.elf      busy loop copying input PB0 to output PA6 (ADR-003 step 2)
# Deterministic build: no timestamps or absolute paths end up in the ELFs
# (-ffile-prefix-map, no debug info by default).

CC      := arm-none-eabi-gcc
SIZE    := arm-none-eabi-size
BUILD   := build
PROGS   := blinky busyloop copy
TARGETS := $(PROGS:%=$(BUILD)/%.elf)

CFLAGS  := -mcpu=cortex-m3 -mthumb -Os -std=c11 -Wall -Wextra -Werror \
           -ffreestanding -ffunction-sections -fdata-sections \
           -ffile-prefix-map=$(CURDIR)=.
LDFLAGS := -nostdlib -nostartfiles -T stm32f103c8.ld -Wl,--gc-sections \
           -Wl,--build-id=none

.PHONY: all clean
.SECONDARY:

all: $(TARGETS)

$(BUILD)/%.o: %.c | $(BUILD)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/%.elf: $(BUILD)/startup.o $(BUILD)/%.o stm32f103c8.ld
	$(CC) $(CFLAGS) $(LDFLAGS) -Wl,-Map=$(BUILD)/$*.map $(BUILD)/startup.o $(BUILD)/$*.o -o $@
	$(SIZE) $@

$(BUILD):
	mkdir -p $@

clean:
	rm -rf $(BUILD)
