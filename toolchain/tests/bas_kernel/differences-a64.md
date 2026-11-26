# Clang AArch64 kernel object comparison

The host check assembles the exact bytes of the guard include followed by
the bytes of each kernel assembly source, as the two Rust `concat!` calls
do. It compares ELF section type, flags, alignment and size, symbol
records, RELA records and file-backed section contents, including A64
instruction words, padding and literal pools. The comparison accounts for
the representations below.

| Scope | Accepted difference | Specification |
| --- | --- | --- |
| ELF tables | Section and symbol indices and table placement can differ. Clang uses one string table for both section and symbol names; bas emits separate tables. The check resolves indices to names and verifies both string-table roles. | ELF gABI §§2, 3, 4 and 5, especially `e_shstrndx` and the symbol table's `sh_link` |
| Defined local relocation targets | Clang can emit an unnamed local `STT_SECTION` symbol with the destination's section offset in the addend. bas can name the defined local label instead. The check requires the exact same relocated section, field offset, relocation type, target section and combined symbol value and addend. Other references use exact symbol names and addends. | ELF gABI §§5.2, 5.3 and 6, symbol values and relocation |
| Temporary labels | bas can retain defined local `.L` symbols for private and generated labels. Clang omits them. The check verifies their binding and type and compares the destination of any relocation that uses one. | ELF gABI §§5.2 and 5.3, local symbols and symbol types |
| Assembly-time constants | Clang retains local absolute `.equ` and `.set` symbols. bas evaluates them while assembling. The check allows the omission only for those declared names with local `STT_NOTYPE`, default visibility and `SHN_ABS`. It checks the emitted instructions, data and relocations that use the constants. | ELF gABI §§3, 5.2, 5.3 and 5.5, absolute symbol values and local binding |
| Boot literal loads | When Clang uses X-register MOVZ or MOVN to implement `ldr Xn, =constant`, bas can load that same 64-bit value from its pool. The check decodes both instructions and requires the same destination register and value. It projects out only unrelocated slots used by those verified loads and adjusts subsequent pool offsets and the matching LDR displacements. The remaining pool bytes and every relocation still match, including field offset in the resulting layout, type, symbol and addend. | Arm ARM DDI 0487 M.b §§C6.2.216, C6.2.283 and C6.2.284 |
