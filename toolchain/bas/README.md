# bas

Build the Rust host tool with Cargo from the `toolchain` workspace.
Run `bas [--target x86_64|aarch64] -o OBJECT SOURCE` to write a little-endian
ELF64 relocatable object. The default target is `x86_64` on either host.

## Input syntax

Use lower-case instructions and registers. The x86-64 backend uses Intel
syntax. A semicolon outside a string starts a comment, and AArch64 also
accepts `//`. The initial section is `.text`, where instructions go.
`section NAME` selects `.text`, `.data`, `.rodata`, `.bss`, `.tdata` or `.tbss`.

- `NAME:` defines a label at the current section offset. Labels are local
  by default and can have names like `.Lloop`. Use `global NAME` anywhere to
  give a symbol global binding. A global declaration without a definition
  produces an undefined ELF symbol. `extern NAME` also declares a global
  symbol for references resolved at link time. `.weak NAME` requests weak
  binding for a definition or reference. `.internal`, `.hidden` and
  `.protected NAME` set the ELF visibility (gABI §5.4).
- `.section "NAME", FLAGS, @TYPE` selects a named section with explicit ELF
  attributes. Use `a` for allocated data, `aw` for writable data, `ax` for
  executable code or `awT` for thread-local data. The type is `progbits`
  or `nobits`. Repeated explicit selections must agree.
- `.alias NAME, TARGET` defines an additional symbol at the target's section
  and offset, with the target's type and size. The alias carries its own
  binding and visibility. The target must resolve to a label in the object.
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
  NUL after each string. Escapes are `\`, `\"`, `\0`, `\n`, `\r`, `\t`
  and `\xHH`.
- `.zero COUNT` fills `COUNT` bytes with `0x00`. `align BYTES[, FILL]` aligns
  to a positive power of two and raises the section's ELF alignment.
  The default fill is `0x90` in executable x86-64 sections and `0x00` elsewhere. For `.bss` and
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

## x86-64 symbol expressions

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

## AArch64 instructions

`movz` takes a W or X register, a 16-bit immediate, and an optional
`lsl #SHIFT` in steps of 16 within the register width. `svc` takes one
16-bit immediate. Write immediates with `#`. `nop` has no operands.
`br` and `blr` take an X register. `ret` defaults to `x30`.

`b`, `bl`, `b.<cond>`, `cbz`/`cbnz`, `tbz`/`tbnz`, literal `ldr`
and `adr` resolve same-section targets during assembly. The displacement
is measured from the instruction and must fit its signed immediate field.
Except for `adr`, the displacement must be a multiple of four bytes.

ELF64 RELA records name the target symbol and carry its signed byte addend.
The relocated field in the object is zero. The relocation names in this
table use the `R_AARCH64_` prefix. <!-- claim-lint:ok: toolchain/tests/bas/run.py checks the 22 RELA records and the exact instruction and data bytes. -->

| Form | Relocation |
| --- | --- |
| Cross-section or external `bl` / `b` | `CALL26` / `JUMP26` |
| Cross-section or external `b.<cond>`, `cbz`, `cbnz` | `CONDBR19` |
| Cross-section or external `tbz`, `tbnz` | `TSTBR14` |
| Cross-section or external literal `ldr` / `adr` | `LD_PREL_LO19` / `ADR_PREL_LO21` |
| `adrp x0, symbol`, including same-section targets | `ADR_PREL_PG_HI21` |
| `add x0, x0, :lo12:symbol` | `ADD_ABS_LO12_NC` |
| Scaled load/store, e.g. `ldr x1, [x0, :lo12:symbol]` | `LDST{8,16,32,64,128}_ABS_LO12_NC` by access width |
| `adrp x0, :got:symbol` / `ldr x0, [x0 :got_lo12:symbol]` | `ADR_GOT_PAGE` / `LD64_GOT_LO12_NC` |
| ADD with `:tprel_hi12:` / `:tprel_lo12_nc:` | `TLSLE_ADD_TPREL_HI12` / `TLSLE_ADD_TPREL_LO12_NC` |
| `dq symbol` / `dd symbol` / `dd symbol - .` | `ABS64` / `ABS32` / `PREL32` |

GOT addends must be zero per AAELF64 §5.7.3.3. <!-- claim-lint:ok: toolchain/tests/bas/run.py checks the located refusal of a nonzero GOT addend. -->
TLS operands require an `STT_TLS` symbol. Labels
in `.tdata` and `.tbss` default to that type. Use `extern` and
`.type NAME, @tls` for an undefined TLS symbol.

## Host checks

Build the native Rust binaries and pass their paths to the host runner.

```sh
cd toolchain
cargo build --release --locked -p bas -p elfcheck
python3 tests/bas/run.py --bas "$PWD/target/release/bas" --elfcheck "$PWD/target/release/elfcheck"
python3 tests/bas/run.py --machine aarch64 --bas "$PWD/target/release/bas" --elfcheck "$PWD/target/release/elfcheck"
cargo test --locked -p bas
```

The runner checks the x86-64 fixtures and the AArch64 branch corpus with
the independent elfcheck. The byte expectations cite the pinned ISA tables
in `toolchain/tests/bas/` and `toolchain/corpus/assembler-aarch64/`. The Rust
test exercises malformed source on both targets.
