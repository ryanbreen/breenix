# Clang AArch64 kernel object comparison

The host check assembles the exact bytes of the guard include followed by
the bytes of each kernel assembly source, as the two Rust `concat!` calls
do. It compares ELF section type, flags, alignment and size, symbol
records, RELA records and all file-backed section contents, including A64
instruction words, padding and literal pools.

| Scope | Accepted difference | Specification |
| --- | --- | --- |
| ELF tables | Section and symbol indices and table placement can differ. Clang uses one string table for both section and symbol names; bas emits separate tables. The check resolves indices to names and verifies both string-table roles. | ELF gABI §§2, 3, 4 and 5, especially `e_shstrndx` and the symbol table's `sh_link` |
| Defined local relocation targets | Clang can emit an unnamed local `STT_SECTION` symbol with the destination's section offset in the addend. bas can name the defined local label instead. The check requires the exact same relocated section, field offset, relocation type, target section and combined symbol value and addend. Other references use exact symbol names and addends. | ELF gABI §§5.2, 5.3 and 6, symbol values and relocation |
| Temporary labels | bas can retain defined local `.L` symbols for private and generated labels. Clang omits them. The check verifies their binding and type and compares the destination of any relocation that uses one. | ELF gABI §§5.2 and 5.3, local symbols and symbol types |
