# NASM kernel object comparison

The host check compares the four sources from `kernel/build.rs`. It decodes the
trampoline in separate 16-, 32- and 64-bit ranges and compares the rest of
`.rodata` as data. It resolves symbol and relocation indices to their names.

| Scope | Accepted difference | Specification |
| --- | --- | --- |
| 64-bit entry sections | NASM can use rel8 for local JMP and Jcc. bas uses rel32. Instruction ordinal determines the destination and the position of each symbol. A relocated CALL must still match at the same instruction and field, with the same type, symbol and addend. | Intel SDM Vol. 2A, JMP and Jcc |
| Syscall entry, selectors 0x2b and 0x33 | NASM uses B8 id to write EAX and clear the upper half of RAX. bas uses REX.W C7 /0 id to sign-extend the same positive constant into RAX. | Intel SDM Vol. 1 §3.4.1.1 and Vol. 2B, MOV |
| ELF bookkeeping | NASM includes the local FILE and SECTION symbols for the input and its payload section. bas includes an implicit empty AX `.text`. For the entry RELA sections, bas sets SHF_INFO_LINK. Both index the same source section through sh_info. | ELF gABI §3, section flags and section links, and §5, symbol types |

The comparison uses instruction bytes outside the forms in the table. It
checks the type, flags and alignment of `.text.entry` and `.rodata`, and
rejects an additional payload section or named symbol.
