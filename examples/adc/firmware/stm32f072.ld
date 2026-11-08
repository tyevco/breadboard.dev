/*
 * STM32F072: 128 KiB flash @ 0x08000000, 16 KiB SRAM @ 0x20000000, the
 * memories of Renode 1.17.0's platforms/cpus/stm32f072.repl.
 *
 * .status is placed first in RAM so that the host (examples/adc/
 * renode_adc_test.py, and later the Renode engine) finds the firmware's
 * status block at a fixed address, 0x20000000, without parsing the ELF. It
 * is NOLOAD and zeroed by Reset_Handler.
 */
ENTRY(Reset_Handler)

MEMORY
{
    FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 128K
    RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 16K
}

_estack = ORIGIN(RAM) + LENGTH(RAM);

SECTIONS
{
    .isr_vector :
    {
        . = ALIGN(4);
        KEEP(*(.isr_vector))
        . = ALIGN(4);
    } > FLASH

    .text :
    {
        . = ALIGN(4);
        *(.text*)
        *(.rodata*)
        . = ALIGN(4);
    } > FLASH

    .status (NOLOAD) :
    {
        _sstatus = .;
        KEEP(*(.status))
        . = ALIGN(4);
        _estatus = .;
    } > RAM

    _sidata = LOADADDR(.data);

    .data :
    {
        . = ALIGN(4);
        _sdata = .;
        *(.data*)
        . = ALIGN(4);
        _edata = .;
    } > RAM AT > FLASH

    .bss (NOLOAD) :
    {
        . = ALIGN(4);
        _sbss = .;
        *(.bss*)
        *(COMMON)
        . = ALIGN(4);
        _ebss = .;
    } > RAM

    /DISCARD/ : { *(.ARM.exidx*) *(.comment) }
}

ASSERT(_sstatus == ORIGIN(RAM), ".status must start at 0x20000000")
