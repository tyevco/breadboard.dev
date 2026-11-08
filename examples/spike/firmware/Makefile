# Bare-metal STM32F103C8 blinky (PA5 toggled by SysTick every 50 ms).
# Output: build/blinky.elf. Deterministic build: no timestamps or absolute
# paths end up in the ELF (-ffile-prefix-map, no debug info by default).

CC      := arm-none-eabi-gcc
SIZE    := arm-none-eabi-size
BUILD   := build
TARGET  := $(BUILD)/blinky.elf

CFLAGS  := -mcpu=cortex-m3 -mthumb -Os -std=c11 -Wall -Wextra -Werror \
           -ffreestanding -ffunction-sections -fdata-sections \
           -ffile-prefix-map=$(CURDIR)=.
LDFLAGS := -nostdlib -nostartfiles -T stm32f103c8.ld -Wl,--gc-sections \
           -Wl,-Map=$(BUILD)/blinky.map -Wl,--build-id=none

SRCS    := startup.c blinky.c
OBJS    := $(SRCS:%.c=$(BUILD)/%.o)

.PHONY: all clean

all: $(TARGET)

$(BUILD)/%.o: %.c | $(BUILD)
	$(CC) $(CFLAGS) -c $< -o $@

$(TARGET): $(OBJS) stm32f103c8.ld
	$(CC) $(CFLAGS) $(LDFLAGS) $(OBJS) -o $@
	$(SIZE) $@

$(BUILD):
	mkdir -p $@

clean:
	rm -rf $(BUILD)
