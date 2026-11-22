# bas

Build the host tool from `bas.c` and `../common/elf64.c` with a C11 compiler.
Run it as `bas -o OBJECT SOURCE`. Output is a little-endian x86-64 ELF
relocatable object.

The 25.3a1 dialect uses lower-case Intel-style register and instruction
names. Semicolons introduce comments. `section .text`, `section .data`,
`section .rodata`, and `section .bss` select a section by name. The initial
section is `.text`. Instructions go in `.text`. Write `global NAME` before
`NAME:` to give a label global binding. A declaration without a label produces
an undefined global symbol.

- `mov r32, IMM` loads a 32-bit immediate into a general-purpose register.
  Literals can be signed or unsigned decimal or hexadecimal with a `0x` prefix.
- `call DISP` takes a signed 32-bit byte displacement from the following
  instruction. Write it as a decimal or hexadecimal integer.
- `ret` and `syscall` take no operands.

Local labels, symbolic calls, more operand widths, data directives and
relocations belong to the later assembler slices. The ARM64 host build
still targets x86-64. AArch64 instruction encoding, object metadata and
relocations are A1 work.

Run `python3 toolchain/tests/bas/run.py --clang /absolute/path/to/clang` on
macOS to assemble the fixture and check its bytes, sections and global symbols.
The runner also passes bas's object to the independent elfcheck.
