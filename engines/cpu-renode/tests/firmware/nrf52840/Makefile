# nRF52840 bare-metal gate firmware (Phase 1 M2, ADR-008 D3/D5), used by
# engines/cpu-renode/tests/nrf52840.rs and examples/nrf-feedback:
#   build/nrf_blinky.elf       RTC0 toggles P0.13 every 1638 ticks (nrf-blinky)
#   build/nrf_feedback.elf     RTC0 -> P0.13; GPIOTE IN P0.11 copied to P0.15 (nrf-feedback)
#   build/nrf_ppi.elf          TIMER1 COMPARE0 -> PPI -> GPIOTE OUT P0.14; ISR P0.16 (nrf-ppi)
#   build/nrf_busywait.elf     10 ms nrfx busy-wait on P0.13, 10 ms RTC sleep on P0.14 (nrf-busywait)
#   build/nrf_timer_sleep.elf  one TIMER0 compare, then a long sleep (ADR-008 D6)
#   build/nrf_pincnf.elf       PIN_CNF configurations for the end-of-run audit (D4)
#   build/nrf_saadc_idle.elf   WFI forever, no interrupt: the SAADC register tests (M8, D7)
#   build/nrf_saadc_burst.elf  RTC0-paced 8x BURST on AIN0, VDD, UARTE0 lines (nrf-saadc, D7/D8)
# Built against the vendored nrfx MDK and CMSIS-Core headers in vendor/
# (scripts/nrf_vendor_headers.sh), with the pinned arm-none-eabi-gcc.
# Deterministic build: no timestamps or absolute paths end up in the ELFs.

CC      := arm-none-eabi-gcc
SIZE    := arm-none-eabi-size
BUILD   := build
LDSCRIPT := nrf52840.ld
PROGS   := nrf_blinky nrf_feedback nrf_ppi nrf_busywait nrf_timer_sleep nrf_pincnf nrf_saadc_idle nrf_saadc_burst
TARGETS := $(PROGS:%=$(BUILD)/%.elf)
HEADERS := nrf_gate.h $(wildcard vendor/nrfx/*.h vendor/cmsis/*.h vendor/cmsis/m-profile/*.h)

CFLAGS  := -mcpu=cortex-m4 -mthumb -mfloat-abi=soft -Os -std=c11 -Wall -Wextra -Werror \
           -ffreestanding -ffunction-sections -fdata-sections -DNRF52840_XXAA \
           -Ivendor/nrfx -Ivendor/cmsis \
           -ffile-prefix-map=$(CURDIR)=.
LDFLAGS := -nostdlib -nostartfiles -T $(LDSCRIPT) -Wl,--gc-sections \
           -Wl,--build-id=none

.PHONY: all clean
.SECONDARY:

all: $(TARGETS)

$(BUILD)/%.o: %.c $(HEADERS) | $(BUILD)
	$(CC) $(CFLAGS) -c $< -o $@

$(BUILD)/%.elf: $(BUILD)/startup.o $(BUILD)/%.o $(LDSCRIPT)
	$(CC) $(CFLAGS) $(LDFLAGS) -Wl,-Map=$(BUILD)/$*.map $(BUILD)/startup.o $(BUILD)/$*.o -o $@
	$(SIZE) $@

$(BUILD):
	mkdir -p $@

clean:
	rm -rf $(BUILD)
