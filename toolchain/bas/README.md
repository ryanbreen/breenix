# bas

Build the host tool from `bas.c` and `../common/elf64.c` with a C11 compiler.
Run `bas -o OBJECT SOURCE` to write a little-endian x86-64 ELF relocatable object.

## Input syntax

Use lower-case Intel-style instructions and registers. A semicolon outside a
string starts a comment. The initial section is `.text`, where instructions go.
`section NAME` selects `.text`, `.data`, `.rodata`, `.bss`, `.tdata` or `.tbss`.

- `NAME:` defines a label at the current section offset. Labels are local
  by default and can have names like `.Lloop`. Use `global NAME` anywhere to
  give a symbol global binding. A global declaration without a definition
  produces an undefined ELF symbol. `extern NAME` also declares a global
  symbol for references resolved at link time.
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
  computes the displacement from the end of the instruction to the label.
  `eip` selects the 32-bit address-size override.
- Direct `call`, `jmp` and the flag-based conditional jumps use rel32.
  An integer is the signed displacement from the end of the instruction.
  A label with an optional signed addend can refer forward or backward
  within the same section or refer to a symbol resolved at link time.
  Indirect `call` and `jmp` take a 64-bit register or memory operand.
  `ret` optionally takes an unsigned 16-bit stack adjustment.
- `db`, `dw`, `dd` and `dq` emit lists of 1-, 2-, 4- and 8-byte
  little-endian integers, respectively. The aliases are `.byte`, `.word`,
  `.long` and `.quad`. Integers accept decimal and `0x` hexadecimal.
  Byte lists and `.ascii` can emit quoted strings. `.asciz` adds a
  NUL after each string. Escapes are `\\`, `\"`, `\0`, `\n`, `\r`, `\t`
  and `\xHH`.
- `.zero COUNT` fills `COUNT` bytes with `0x00`. `align BYTES[, FILL]` aligns
  to a positive power of two and raises the section's ELF alignment.
  The default fill is `0x90` in `.text` and `0x00` elsewhere. For `.bss` and
  `.tbss`, use a fill of `0x00`. They record their sizes as `SHT_NOBITS`. <!-- claim-lint:ok: toolchain/tests/bas/run.py checks the .zero directive and NOBITS layout. -->
- `.type NAME, @function`, `@object`, `@tls` or `@notype` sets the ELF symbol type.
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

## Floating point, synchronization and strings

- SSE/SSE2 accepts `xmm0` through `xmm15`, scalar `ss`/`sd` and packed
  `ps`/`pd` operations, and aligned and unaligned moves. The supported forms
  cover arithmetic, comparisons, scalar conversions, register transfers,
  shuffles and common packed integer operations. Use `xmmword` for explicit
  128-bit memory operands and `dword` or `qword` to select an integer source
  width for `cvtsi2ss` and `cvtsi2sd`.
- x87 uses `st(0)` through `st(7)` (also `st0` through `st7`). It supports
  stack and memory loads and stores, arithmetic, comparisons, integer
  conversions, and control and status words. Use `tbyte` for an 80-bit
  memory operand and an explicit width where the opcode depends on it.
- `lock` applies to the memory-destination forms of the supported
  read-modify-write instructions, including `cmpxchg`, `xadd` and `xchg`.
  `lfence`, `sfence` and `mfence` have no operands.
- String instructions `movs`, `stos`, `lods`, `cmps` and `scas` use the
  suffix `b`, `w`, `d` or `q` for implicit operands. Use `rep` for
  moves, stores and loads, and `repe` or `repne` for comparisons and scans.

## Symbol expressions

Fields support a symbol with a signed integer addend. Unmodified
same-section PC-relative references are resolved while assembling.
Cross-section and undefined global references create ELF64 RELA records.

| Form | Relocation |
| --- | --- |
| `dq symbol` | `R_X86_64_64` |
| `dd symbol` or an absolute address with a 32-bit base/index | `R_X86_64_32` |
| `dd symbol@32s` or an absolute address with default 64-bit address size | `R_X86_64_32S` |
| `call symbol`, `jmp symbol`, conditional branch, or `[rip + symbol]` | `R_X86_64_PC32` |
| `call symbol@plt` or `jmp symbol@plt` | `R_X86_64_PLT32` |
| `[rip + symbol@gotpcrel]` | `R_X86_64_GOTPCREL` |
| `fs:[symbol@tpoff]` | `R_X86_64_TPOFF32` |

The PC-relative addend accounts for the bytes from the relocated field to
the end of the instruction. Labels in `.tdata` and `.tbss` default to
`STT_TLS`. Declare an undefined TLS symbol with `extern` and `.type NAME, @tls`
before using `@tpoff`.

The ARM64 host and guest builds emit x86-64 objects. AArch64 encodings,
object metadata and relocations are A1 work.

## Host checks

Run `python3 toolchain/tests/bas/run.py --clang /absolute/path/to/clang` on
macOS. The runner checks the exit42, addressing, integer, supplemental encoding
and relocation fixtures on both host architectures and under ASan and UBSan.
The independent elfcheck validates
the resulting objects. The byte expectations cite the pinned ISA tables in
`toolchain/tests/bas/`.
