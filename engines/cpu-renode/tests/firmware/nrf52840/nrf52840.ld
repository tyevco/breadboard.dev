/* nRF52840 (QIAA) memory map for Breadboard's bare-metal gate firmware:
 * 1 MB flash at 0, 256 KB RAM at 0x20000000 (as Renode 1.17.0's
 * platforms/cpus/nrf52840.repl maps them). */
MEMORY
{
  FLASH (rx) : ORIGIN = 0x00000000, LENGTH = 1024K
  RAM (rwx)  : ORIGIN = 0x20000000, LENGTH = 256K
}
_estack = ORIGIN(RAM) + LENGTH(RAM);
SECTIONS
{
  .text : { KEEP(*(.isr_vector)) *(.text*) *(.rodata*) . = ALIGN(4); } > FLASH
  _sidata = LOADADDR(.data);
  .data : { _sdata = .; *(.data*) . = ALIGN(4); _edata = .; } > RAM AT > FLASH
  .bss (NOLOAD) : { _sbss = .; *(.bss*) *(COMMON) . = ALIGN(4); _ebss = .; } > RAM
}
