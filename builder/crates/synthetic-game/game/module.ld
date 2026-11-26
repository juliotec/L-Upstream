/* The synthetic game's module as an NSO holds one (src/lib.rs `nso_from_elf`): three segments from offset 0, .text
   (with the MOD0 header and the PLT), .rodata (the dynamic symbols and their strings, back to back, as the read stage
   counts the symbols by the distance between the two, and the relocations), .data (the GOT, the dynamic section) and
   the bss after it. Linked as a shared object, as the console's modules are, with its imports through the PLT. */
PHDRS {
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
    dynamic PT_DYNAMIC FLAGS(6);
}

SECTIONS {
    . = 0;
    .text : { KEEP(*(.text.crt0)) *(.text .text.*) } :text
    .plt : { *(.plt) } :text
    . = ALIGN(0x1000);
    .rodata : { *(.rodata .rodata.*) } :rodata
    .dynsym : { *(.dynsym) } :rodata
    .dynstr : { *(.dynstr) } :rodata
    .hash : { *(.hash) } :rodata
    .gnu.hash : { *(.gnu.hash) } :rodata
    .rela.dyn : { *(.rela.dyn) } :rodata
    .rela.plt : { *(.rela.plt) } :rodata
    . = ALIGN(0x1000);
    .data : { *(.data .data.*) } :data
    .got : { *(.got) } :data
    .got.plt : { *(.got.plt) } :data
    .dynamic : { *(.dynamic) } :data :dynamic
    . = ALIGN(8);
    __bss_start = .;
    .bss : { *(.bss .bss.*) *(COMMON) } :data
    . = ALIGN(8);
    __bss_end = .;
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
