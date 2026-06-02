OUTPUT_ARCH(loongarch)
ENTRY(_start)

PHYS_ADDR = 0x00200000;
BASE_ADDRESS = 0xffff800000200000;
KERNEL_OFFSET = BASE_ADDRESS - PHYS_ADDR;

SECTIONS
{
    . = BASE_ADDRESS;
    skernel = .;

    stext = .;
    .text : AT(ADDR(.text) - KERNEL_OFFSET) {
        *(.text.entry)
        . = ALIGN(4K);
        srfill = .;
        KEEP(*(.text.__rfill))
        . = ALIGN(4K);
        erfill = .;
        *(.text .text.*)
    }

    . = ALIGN(4K);
    etext = .;
    srodata = .;
    .rodata : AT(ADDR(.rodata) - KERNEL_OFFSET) {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    . = ALIGN(4K);
    erodata = .;
    sdata = .;
    .data : AT(ADDR(.data) - KERNEL_OFFSET) {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    . = ALIGN(4K);
    edata = .;
    sbss_with_stack = .;
    .bss : AT(ADDR(.bss) - KERNEL_OFFSET) {
        *(.bss.stack)
        sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
    }

    . = ALIGN(4K);
    ebss = .;
    ekernel = .;

    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
    }
}
