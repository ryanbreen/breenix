/* Userspace program linker script for Rust std programs
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
    stack PT_GNU_STACK FLAGS(6);
}

SECTIONS {
    . = 0x40000000 + SIZEOF_HEADERS; /* Start at 1GB (userspace area) */

    .text : ALIGN(4K) {
        *(.text .text.*)
    } :text

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    } :rodata

    .data : ALIGN(4K) {
        *(.data .data.*)
    } :data

    .bss : ALIGN(4K) {
        *(.bss .bss.*)
    } :data

    /* Thread-local storage */
    .tdata : ALIGN(4K) {
        *(.tdata .tdata.*)
    } :data :tls

    .tbss : ALIGN(4K) {
        *(.tbss .tbss.*)
    } :data :tls
}
