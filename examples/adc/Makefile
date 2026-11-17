# examples/adc: the ADR-003 Part C ADC platform (approver decision 4,
# Amendment 5). Run from the repo root inside WSL / Linux:
#   make -C examples/adc firmware   build firmware/build/{adc,adc_hwtrig}.elf
#   make -C examples/adc test       Renode-only External Control test of
#                                   Renode's stock ADC model
#                                   (renode_adc_test.py, ADR-003 Amendment
#                                   5), transcripts in out/adc at the repo
#                                   root. Run by hand only since Phase 2 H6
#                                   (ADR-015 D9 removed the stock path);
#                                   renode_adc_test.py stays the External
#                                   Control client of bb_adc_test.py and
#                                   examples/nrf-saadc/bb_saadc_test.py
#   make -C examples/adc model-test Renode-only tests of Breadboard's ADC
#                                   model (bb_adc_test.py, ADR-005),
#                                   transcripts in out/adc-model
SHELL := /bin/bash
export PATH := $(HOME)/.local/bin:$(PATH)
PYTHON ?= python3
ROOT   := $(abspath $(CURDIR)/../..)

.PHONY: all firmware test model-test clean

all: test

firmware:
	$(MAKE) -C firmware

test: firmware
	$(PYTHON) renode_adc_test.py --out $(ROOT)/out/adc

model-test: firmware
	$(PYTHON) bb_adc_test.py --out $(ROOT)/out/adc-model

clean:
	$(MAKE) -C firmware clean
