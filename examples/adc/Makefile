# examples/adc: the ADR-003 Part C ADC platform (approver decision 4,
# Amendment 5). Run from the repo root inside WSL / Linux:
#   make -C examples/adc firmware   build firmware/build/{adc,adc_hwtrig}.elf
#   make -C examples/adc test       Renode-only External Control test
#                                   (renode_adc_test.py), transcripts in
#                                   out/adc at the repo root
SHELL := /bin/bash
export PATH := $(HOME)/.local/bin:$(PATH)
PYTHON ?= python3
ROOT   := $(abspath $(CURDIR)/../..)

.PHONY: all firmware test clean

all: test

firmware:
	$(MAKE) -C firmware

test: firmware
	$(PYTHON) renode_adc_test.py --out $(ROOT)/out/adc

clean:
	$(MAKE) -C firmware clean
