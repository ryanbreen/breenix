/* Userspace linker script for programs with hosted startup and unwind metadata
 *
 * This linker script is required to place userspace programs at 0x40000000
 * to avoid conflicts with kernel page table mappings at lower addresses.
 */
ENTRY(_start)

PHDRS {
    text PT_LOAD FILEHDR PHDRS FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
    tls PT_TLS FLAGS(4);
    /* LSB Core 5.0 §11.2/Table 11-1; ELF gABI Table 7.3. */
    eh_frame PT_GNU_EH_FRAME FLAGS(4);
    stack PT_GNU_STACK FLAGS(6);
}

SECTIONS {
    . = 0x40000000 + SIZEOF_HEADERS; /* Start at 1GB (userspace area) */

    .text : ALIGN(4K) {
        *(.text .text.*)
    } :text

    /* Keep the R and RW loads on separate pages even when the first
     * section of either load is empty. */
    . = ALIGN(4K);
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
        /* Static references have relocations resolved by this fixed-address link.
         * Their descriptors must remain read-only, like the tables they name. */
        *(.data.rel.ro .data.rel.ro.*)
    } :rodata

    /* LSB Core 5.0 Table 10-4 and §10.6. Only the index is described
     * by PT_GNU_EH_FRAME (GNU ld --eh-frame-hdr). */
    .eh_frame_hdr : {
        *(.eh_frame_hdr)
    } :rodata :eh_frame

    .eh_frame : {
        *(.eh_frame .eh_frame.*)
    } :rodata

    .gcc_except_table : {
        *(.gcc_except_table .gcc_except_table.*)
    } :rodata

    . = ALIGN(4K);
    .data : ALIGN(4K) {
        *(.data .data.*)
    } :data

    /* ELF gABI Table 3.8. KEEP preserves startup entries through GC,
     * and PROVIDE_HIDDEN exposes their bounds only to the static link
     * (GNU ld §§3.6.4.4 and 3.5.4). */
    .preinit_array : {
        PROVIDE_HIDDEN(__preinit_array_start = .);
        KEEP(*(.preinit_array .preinit_array.*))
        PROVIDE_HIDDEN(__preinit_array_end = .);
    } :data

    .init_array : {
        PROVIDE_HIDDEN(__init_array_start = .);
        KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*)))
        KEEP(*(.init_array))
        PROVIDE_HIDDEN(__init_array_end = .);
    } :data

    .fini_array : {
        PROVIDE_HIDDEN(__fini_array_start = .);
        KEEP(*(SORT_BY_INIT_PRIORITY(.fini_array.*)))
        KEEP(*(.fini_array))
        PROVIDE_HIDDEN(__fini_array_end = .);
    } :data

    /* Thread-local storage template, ahead of .bss so .bss stays out of the
     * file. The C library reads it through PT_TLS. */
    .tdata : {
        *(.tdata .tdata.*)
    } :data :tls

    .tbss : {
        *(.tbss .tbss.*)
    } :data :tls

    .bss : ALIGN(4K) {
        *(.bss .bss.*)
    } :data
}
