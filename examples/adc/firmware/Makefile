# Bare-metal STM32F072 (Cortex-M0) firmware for examples/adc (ADR-003 Part C,
# approver decision 4; Amendment 5). Runs on Renode 1.17.0's
# platforms/cpus/stm32f072.repl (ADC = Analog.STM32F0_ADC).
#   build/adc.elf         TIM3 update ISR (1 ms) starts each conversion of
#                         ADC channel 1; analog watchdog [1.0 V, 2.0 V] drives
#                         PA5 (ALARM); status block at 0x20000000
#   build/adc_hwtrig.elf  the same with EXTSEL = TIM3_TRGO, EXTEN = rising and
#                         TIM3 at 2 ms (the hardware trigger path; Renode's
#                         model ignores EXTSEL, see adc.c)
#   build/adc_board.elf   adc.elf plus the board drive (PA4 toggles every 50
#                         TIM3 updates) and code bits 4 and 8 on PA6/PA7, for
#                         examples/adc/board.cir (ADR-003 plan step 9)
# Deterministic build: no timestamps or absolute paths end up in the ELF.

CC      := arm-none-eabi-gcc
SIZE    := arm-none-eabi-size
BUILD   := build
LDSCRIPT := stm32f072.ld

CFLAGS  := -mcpu=cortex-m0 -mthumb -Os -std=c11 -Wall -Wextra -Werror \
           -ffreestanding -ffunction-sections -fdata-sections \
           -ffile-prefix-map=$(CURDIR)=.
LDFLAGS := -nostdlib -nostartfiles -T $(LDSCRIPT) -Wl,--gc-sections \
           -Wl,--build-id=none

.PHONY: all clean
.SECONDARY:

all: $(BUILD)/adc.elf $(BUILD)/adc_hwtrig.elf $(BUILD)/adc_board.elf

$(BUILD)/startup.o: startup.c | $(BUILD)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/adc.o: adc.c | $(BUILD)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/adc_hwtrig.o: adc.c | $(BUILD)
	$(CC) $(CFLAGS) -DADC_HW_TRIGGER -c $< -o $@

$(BUILD)/adc_board.o: adc.c | $(BUILD)
	$(CC) $(CFLAGS) -DADC_BOARD -c $< -o $@

$(BUILD)/%.elf: $(BUILD)/startup.o $(BUILD)/%.o $(LDSCRIPT)
	$(CC) $(CFLAGS) $(LDFLAGS) -Wl,-Map=$(BUILD)/$*.map $(BUILD)/startup.o $(BUILD)/$*.o -o $@
	$(SIZE) $@

$(BUILD):
	mkdir -p $@

clean:
	rm -rf $(BUILD)
