# bas

Build the host tool from `bas.c` and `../common/elf64.c` with a C11 compiler.
Run `bas -o OBJECT SOURCE` to write a little-endian x86-64 ELF relocatable object.

## Input syntax

Use lower-case Intel-style instructions and registers. A semicolon outside a
string starts a comment. The initial section is `.text`, where instructions go.
`section NAME` selects `.text`, `.data`, `.rodata` or `.bss`.

- `NAME:` defines a label at the current section offset. Labels are local
  by default and can have names like `.Lloop`. Use `global NAME` anywhere to
  give a symbol global binding. A global declaration without a definition
  produces an undefined ELF symbol.
- `mov` supports 8-, 16-, 32- and 64-bit GPR pairs and GPR loads and stores.
  A register implies the memory width. Write `byte`, `word`, `dword` or
  `qword` before the address to make that width explicit. An optional `ptr`
  can follow the width. Use an explicit width for an immediate store. A 64-bit
  register move uses a sign-extended imm32 when the value fits and an imm64
  otherwise. A 64-bit immediate store requires a value that fits the
  sign-extended imm32 form.
- Write addresses as `[BASE + INDEX*SCALE + DISP]`, with any of those terms
  omitted. Scales are 1, 2, 4 or 8. Address registers are uniformly 64-bit or
  uniformly 32-bit within an address. Write `fs:[...]` or `gs:[...]` for a
  segment base. The other four segment prefixes encode but have no effect
  on ordinary memory addresses in 64-bit mode.
- `[rip + DISP]` uses a literal displacement. `[rip + LABEL + ADDEND]`
  computes the displacement from the end of the instruction to a label in
  the same section. `eip` selects the 32-bit address-size override.
- Direct `call`, `jmp` and the flag-based conditional jumps use rel32.
  An integer is the signed displacement from the end of the instruction.
  A label with an optional signed addend can refer forward or backward
  within the same section. Indirect `call` and `jmp` take a 64-bit register or
  memory operand. `ret` optionally takes an unsigned 16-bit stack adjustment.
- `db`, `dw`, `dd` and `dq` emit lists of 1-, 2-, 4- and 8-byte
  little-endian integers, respectively. The aliases are `.byte`, `.word`,
  `.long` and `.quad`. Integers accept decimal and `0x` hexadecimal.
  Byte lists and `.ascii` can emit quoted strings. `.asciz` adds a
  NUL after each string. Escapes are `\\`, `\"`, `\0`, `\n`, `\r`, `\t`
  and `\xHH`.
- `.zero COUNT` fills `COUNT` bytes with `0x00`. `align BYTES[, FILL]` aligns
  to a positive power of two and raises the section's ELF alignment.
  The default fill is `0x90` in `.text` and `0x00` elsewhere. `.bss` accepts
  `0x00` data and records its size as `SHT_NOBITS` in the object. <!-- claim-lint:ok: toolchain/tests/bas/run.py checks the fixture using this directive. -->
- `.type NAME, @function`, `@object` or `@notype` sets the ELF symbol type.
  `.size NAME, SIZE` accepts a nonnegative integer or a difference between
  offsets in one section, such as `. - NAME`. A dot in that expression
  denotes the directive's offset.

## Integer instructions

- `movsx` and `movzx` extend a byte or word into a wider register. `movsxd`
  extends a doubleword into a 64-bit register. Specify the width of a memory
  source. The implicit-register forms are `cbw`, `cwde`, `cdqe`, `cwd`,
  `cdq` and `cqo`.
- `add`, `adc`, `sub`, `sbb`, `and`, `or`, `xor` and `cmp` take a
  register with a register or memory operand, or an immediate source.
  `test` takes a register or immediate source. `inc`, `dec`, `not`, `neg`,
  `mul`, `div` and `idiv` take one register or memory operand. `imul`
  also accepts a register destination with a register or memory source,
  optionally followed by an immediate. `imul REG, IMM` uses REG as both
  destination and source. Arithmetic immediates for a 64-bit destination
  must fit as sign-extended imm32 values.
- `shl` (also `sal`), `shr`, `sar`, `rol`, `ror`, `rcl` and `rcr`
  accept an unsigned byte count or `cl`. Use an explicit width when the
  other operands do not determine the memory width.
- `setcc` takes a byte register or memory operand. `cmovcc` takes a 16-,
  32- or 64-bit register destination and an equal-width register or memory
  source. They use the same flag condition suffixes as `jcc`.
- `push` and `pop` accept 16- or 64-bit registers and memory operands.
  Memory defaults to 64 bits. `push IMM` pushes a sign-extended 8- or
  32-bit immediate as a 64-bit value. `lea` computes a memory address into
  a 16-, 32- or 64-bit register. `leave`, `nop`, `cld`, `std` and
  `syscall` take no operands.

bas resolves same-section references while writing the object.
External references and references between sections require the relocations
scheduled for 25.3b. The ARM64 host build emits x86-64 objects. AArch64
instruction encoding, object metadata and relocations are A1 work.

## Host checks

Run `python3 toolchain/tests/bas/run.py --clang /absolute/path/to/clang` on
macOS. The runner checks the exit42, addressing and integer fixtures on both
host architectures and under ASan and UBSan. The independent elfcheck validates
the resulting objects. The byte expectations cite the pinned ISA tables in
`toolchain/tests/bas/`.
