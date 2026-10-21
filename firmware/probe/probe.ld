/* Stock 1.1.15 only. Code lives in identified final image-format padding.
 * The SDK loader maps the entire rounded KM4 image, including this sector;
 * the actual device loader/MPU remains an explicitly unverified assumption. */
ENTRY(probe_add_version)
stock_json_add = 0x0e00bc71;
stock_json_find = 0x0e00bc3f;
stock_json_string = 0x0e00bcfb;
stock_json_duplicate = 0x0e00b145;
stock_json_append = 0x0e00bc59;
stock_json_delete = 0x0e00b821;
SECTIONS
{
    .probe 0x0e138c80 :
    {
        KEEP(*(.text.probe_add_version))
        *(.text*)
        *(.rodata*)
        . = ALIGN(4);
    }
    .data : { *(.data*) }
    .bss : { *(.bss*) *(COMMON) }
    /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) }
    ASSERT(SIZEOF(.probe) <= 512, "Probe exceeds its reviewed padding slot")
    ASSERT(SIZEOF(.data) == 0, "Probe must not use initialized global state")
    ASSERT(SIZEOF(.bss) == 0, "Probe must not use uninitialized global state")
}
