# E25 specification ledger

The header requirements below come from N1570. The ABI documents fix target
layout and calling conventions. The Clang manual supplies stage-0 builtin
spellings. Use the compiler and linker as black-box executables. Keep Clang
resource headers off the reference include path. Send gaps in a pinned
specification to the E25 coordinator before implementing the affected feature.

## Edition pins

The SHA-256 column identifies the downloaded document bytes. A dash marks an
edition pin whose full publication has no digest recorded here.

| Reference | Pinned edition or publication | SHA-256 |
| --- | --- | --- |
| [WG14 N1570](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n1570.pdf) | C11 committee draft, 2011-04-12 | `208969fec021e4f4ce87a1873765298c73e7bbb5209b1c5868967c7adfc8492e` |
| [ISO/IEC 9899:2018](https://www.iso.org/standard/74528.html) | C17, edition 4, 2018-06 | — |
| [WG14 N2243](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n2243.htm) | C11 clarification requests, version 1.13, October 2017 | `61522ef6c63eb6e68964d8a6b5724c2f9ea3304437367195218f631eaa7998ba` |
| [System V AMD64 psABI](https://raw.githubusercontent.com/wiki/hjl-tools/x86-psabi/x86-64-psABI-1.0.pdf) | AMD64 ABI Draft 1.0, 2018-01-28 | `f5c0e21542fc379090252e18ff3d083d2a5f51da8cca62b0fec41cc330a74641` |
| [ELF gABI](https://gabi.xinuos.com/elf.pdf) | Xinuos ELF Object File Format, version 4.3 draft, 2025-09-04 | `e695f547dbb989e204dde79bc4dc6cfee7de7cca9b65a10208142932a216831b` |
| [Linux Standard Base Core Specification](https://refspecs.linuxfoundation.org/LSB_5.0.0/LSB-Core-generic/LSB-Core-generic.pdf) | 5.0, Generic Part, chapter 11 (E25 ruling 8) | — |
| [Intel SDM](https://cdrdv2-public.intel.com/812392/325462-sdm-vol-1-2abcd-3abcd-4.pdf) | Combined volumes 1–4, 325462-082US, December 2023 | `3b2551f01b4c91dc80e7f9cfb1e9790feeefe503cff15d2922ace049f4d8730f` |
| [AMD APM](https://kib.kiev.ua/x86docs/AMD/AMD64/40332-r4.09.pdf) | AMD publication 40332, combined volumes 1–5, revision 4.09, March 2026, archived PDF | `ae18704f924bb80bc9bbfe9d3c33b97c16cdeaa9c7afccb033d136e86fd88a08` |
| [AAPCS64](https://github.com/ARM-software/abi-aa/releases/download/2025Q1/aapcs64.pdf) | 2025Q1, 2025-04-07 | `11e6b27bb994b23e6a28b7ac27b1437591cfc13c2bf77262fb8f7ea2a0f254b8` |
| [Arm Architecture Reference Manual](https://developer.arm.com/documentation/ddi0487/mb/) | Arm A-profile, DDI 0487, revision M.b | — |
| [POSIX make](https://pubs.opengroup.org/onlinepubs/9799919799.2024edition/utilities/make.html) | POSIX.1-2024, Issue 8, `make` utility | `86663126116573d3b62d89a86807e205030bd2e161beeeddfbdab7c7d25d52b4` |
| [GNU make manual](https://www.gnu.org/software/make//manual/make.pdf) | Edition 0.77, GNU make 4.4.1, 2023-02-26 | `a4bc06026984382815e392159e60a4bc356cac6c5b81e5ae84fa4f58f621f4d3` |
| [GNU C extensions](https://gcc.gnu.org/onlinedocs/gcc-15.2.0/gcc/C-Extensions.html) | GCC 15.2.0 manual, GNU C extensions chapter | `4965dab10bf21e0147e86aa80fca2a92da2c099393f34d0325c913aa3be871f2` |
| [NASM manual](https://www.nasm.us/xdoc/3.02/nasmdoc.pdf) | NASM 3.02, reserved for 25.12 | `a4d1766c9d640663569bcf93feac0657732fe52017002f1dbc797353f6aed34f` |
| [GNU ld manual](https://sourceware.org/binutils/docs-2.45/ld.html) | GNU Binutils 2.45, command-line options and linker script syntax | `f219c3b05e197308c5b60fc2d4dbfc302617389ca5c21501de58a90b3d052e06` |
| [Oracle Solaris `ar(3HEAD)`](https://docs.oracle.com/cd/E86824_01/html/E54772/ar.h-3head.html) | Oracle Solaris 11.3, archive file format | — |
| [Clang Language Extensions](https://releases.llvm.org/22.1.0/tools/clang/docs/LanguageExtensions.html) | Clang 22.1 release manual, builtin varargs, C11 atomics, `offsetof` and floating rounding | `66bb3c0e28f15608750483506cb079d421f9cfb5e01d25b838d921f7efabf860` |

## Support headers

The two ELF targets use 8-bit bytes, 32-bit `int`, and 64-bit `long` and
pointers. AMD64 uses signed plain `char` and x87 extended `long double` in
16 bytes of storage. AAPCS64 uses unsigned plain `char` and IEEE binary128
`long double`. Both align `long double` to 16 bytes.

| Header | N1570 clause | Feature and specification detail |
| --- | --- | --- |
| `stddef.h` | §7.19 | LP64 `size_t` and `ptrdiff_t`, target `wchar_t`, `max_align_t`, `NULL`, and integer-constant `offsetof`. AMD64 psABI §3.1.2 and AAPCS64 §§5.1, 10.1. |
| `stdarg.h` | §§7.16, 7.16.1 | `va_list`, `va_start`, `va_arg`, `va_copy`, and `va_end`. The documented Clang builtins implement AMD64 psABI §3.5.7 and AAPCS64 §§10.1.5, 14. |
| `stdint.h` | §§7.20.1–7.20.4 | Exact, least, fast, pointer and greatest-width types, limits, and promoted literal types. AMD64 psABI §3.1.2 and AAPCS64 §§5.1, 10.1. |
| `limits.h` | §§5.2.4.2.1, 7.10 | LP64 integer bounds, target plain-`char` signedness, and a four-byte multibyte limit for this profile. AMD64 psABI §3.1.2 and AAPCS64 §§5.1, 10.1. |
| `float.h` | §5.2.4.2.2 | Floating-point precision, exponent range and subnormal limits from the compiler's target format. The rounding builtin reports the current mode. The stage-0 profile evaluates at source precision. AMD64 psABI §3.1.2 and AAPCS64 §§5.1, 10.1. |
| `stdbool.h` | §7.18 | `bool`, `true`, `false`, and the definitions indicator. |
| `stdalign.h` | §7.15 | `alignas`, `alignof`, and their definitions indicators. |
| `stdnoreturn.h` | §7.23 | `noreturn` spelling for `_Noreturn`. |
| `iso646.h` | §7.9 | Alternative spellings for logical and bitwise operators. |
| `stdatomic.h` | §§7.17.1–7.17.8 | Memory orders, lock-free scalar types, initialization, fences, generic operations and `atomic_flag`. The stage-0 macros use the documented Clang C11 intrinsics. |

This unit makes the fence and flag functions available inline within a
translation unit. External library symbols under N1570 §7.1.2 and support
for compiler-emitted non-lock-free aggregate runtime calls remain open.
C17 corrections and wider GNU dialect support have their own milestones.

## Host check

Run `toolchain/tests/check-headers.sh /absolute/path/to/clang`. The check
compiles `lp64_static_asserts.c` for both ELF targets under strict C11 warnings
with `-nostdinc`. The native ASan and UBSan run exercises the varargs and
scalar atomic interfaces. Guest execution of the AMD64 and AAPCS64 corpus
belongs to 25.2b.

## Reference corpus

The staged sysroot places libc headers in `include/` and C support headers in
`include-support/` under `/opt/breenix-dev/25.2/`. With `-nostdinc`, add both as
`-isystem` directories in that order so libc's `limits.h` can load the C limits
before adding POSIX names when requested.

Each translation unit in `toolchain/corpus/` names the N1570 clauses it
exercises. The `cases` file lists the source stems in execution order. A
case's `.expect` starts with its decimal exit status and a newline. The
remaining bytes are its expected stdout, which can be empty or contain NUL.
Expectations are written from the clauses before compilation. The variadic
case crosses the integer argument register boundary and consumes a promoted
floating argument from both the original and a copied list. The atomics
case exercises scalar fetch, failed and successful compare-exchange, and flag
operations in one thread.

## Object checker

`toolchain/tests/elfcheck/` checks x86-64 ELF64 relocatable objects against
the pinned gABI 4.3 and psABI 1.0. It covers the ELF header, the section
header table, string tables, the symbol table, RELA entries and section groups.
With `--exec` it checks static executables: the program header table, PT_LOAD
alignment, congruence, order and permissions, the headers in the first load,
the entry point, section placement in loads, PT_PHDR, PT_GNU_STACK and the
PT_TLS template (gABI 3.10, 7.1-7.7; psABI 3.3.3, 5.1). `README.md` there
cites the clause behind each of its 85 checks. Each check has a hand-built
negative control in `fixtures.py` or `fixtures_exec.py`. bas output is checked
against it from plan step 25.3a1, and bld output from 25.4a1.

Linux Standard Base Core 5.0 Table 11-1 assigns `PT_GNU_STACK` its `p_type`
value, 0x6474e551. Section 11.2 defines `p_flags` as the stack permissions.
bld emits a read-write stack and elfcheck checks that it lacks execute permission.

## Static archive inputs

The archive reader accepts the common archive format documented in Solaris
`ar(3HEAD)`, including the `//` long-name table and `/` and `/SYM64/` symbol
indexes. An ordinary nonempty archive needs an index. `--whole-archive`
includes its members in archive order and can read an archive without an index.

`bar r[c][s]` replaces members by basename and appends new members in operand
order. `bar s` rebuilds the index. The writer uses the Solaris ar(3HEAD)
32-bit big-endian index and the long-name table. The index follows member
and ELF symbol-table order and contains named global and weak definitions.
The writer puts `0` in the timestamp, UID and GID fields and `0644` in
the mode field. An
archive that exceeds the 32-bit format limit is rejected. `bnm` reads the
ELF gABI static symbol table in a little-endian ELF64 file or common archive,
with `-A` for input prefixes, `-g` for external symbols, `-P` for
name-first output and `-u` for undefined symbols.

GNU ld 2.45 §2.1 describes the command-line ordering, group and whole-archive
options. bld visits input paths in command-line order. It searches each
ordinary archive to a member-selection fixed point at that position and
repeats that search for the archives between `--start-group` and `--end-group`.
`--no-whole-archive` resets selection to the indexed search. ELF gABI §§5.2
and 5.5 describe weak binding and common storage.

## Static executable layout

`bld -T` accepts the userspace `linker.ld` subset of GNU ld 2.45. It uses
`ENTRY`, the RX/R/RW `PHDRS` with `FILEHDR`, `PHDRS` and `FLAGS`, optional
`PT_TLS`, and a non-executable `PT_GNU_STACK`. `SECTIONS` starts at a
page-aligned base plus `SIZEOF_HEADERS` and selects input section names
with `*(pattern ...)`, `KEEP`, output `ALIGN` and explicit segment names.
Unmatched allocated sections follow their type and flags into the
executable, read-only, writable, TLS, or ordinary zero-fill region.

GNU ld 2.45 §§2.1 and 3.6 describe `--gc-sections` and `KEEP`. Entry and
`KEEP` sections root the relocation graph. ELF gABI §3.9 governs COMDAT
selection and the retention of whole groups. AMD64 psABI §§4.4 and 10.3
govern static relocation arithmetic, GOT slots without relaxation and
variant-II TLS offsets. ELF gABI §7.7 describes the TLS template.

## C conformance fixtures

`toolchain/tests/cconf/` holds expected outputs for N1570 translation phases
1-3 (§§5.1.1.2, 5.2.1.1, 6.4, 6.4.9) and clause 6.10, and the hello subset's
types and constraint diagnostics. They were written from N1570 before bcc
existed. Each fixture cites its clause. The one fixture whose result N1570
leaves unspecified (6.10.3.4p4) lists both permitted results. Diagnostics follow bcc's
`file:line:column` convention: physical lines and byte columns, and presumed
names and lines after `#line`. `README.md` there gives the runner contract
for the 25.5 units.

## Compiler translation phases 1–3

`toolchain/bcc/pp.c` follows N1570 §§5.1.1.2 and 5.2.1.1 for newline
normalization, trigraphs, splices and comments, and §§6.4–6.4.9 for
preprocessing tokens. Its source encoding is UTF-8, with an optional
leading byte order mark and LF, CRLF or CR line endings. Extended identifier
spellings use universal character names. The lexer preserves their spelling
and leaves code point validation and conversion to later interpretation.
Physical ranges use 1-based byte columns in the original source, including
bytes removed by trigraph replacement and splicing. The lexer records
logical newline tokens and whether horizontal whitespace precedes a token.
It recognizes header names after a leading `#include` or `%:include`.

## Compiler translation phase 4

`toolchain/bcc/pp_expand.c` implements N1570 §§6.10.3–6.10.3.5 and
6.10.8. Arguments used by `#` or `##` retain their spelling. Other arguments
are expanded before substitution. Rescanning uses per-token suppression
sets and includes the remaining source tokens. Token pasting evaluates
left to right, one of the orders allowed by §6.10.3.3p3.

`toolchain/bcc/pp_expr.c` evaluates §6.10.1 expressions with 64-bit
`intmax_t` and `uintmax_t` rules on both LP64 targets. Plain `char` is signed
for x86-64 and unsigned for AArch64, following the AMD64 psABI §3.1.2 and
AAPCS64 §§5.1, 10.1. The execution character encoding is UTF-8. Signed
right shifts sign-extend, and ordinary multibyte character constants pack
their bytes into a target `int`.

`toolchain/bcc/preprocess.c` handles directive and include state under
N1570 §§6.10–6.10.7 and 6.10.9. Quoted includes look next to the current
physical file first. Both forms then search `-I` directories in order and
the bundle's `include-support` and `include` directories. The compiled
bundle root defaults to `/opt/breenix-dev/25.2` and can be set with
`BCC_SYSTEM_ROOT` when building bcc. Phase 4 keeps pragmas in the `-E` token
stream. It retains the original spelling location and the presumed
location used by `#line`, with include notes on diagnostics.

Build the `-E` entry point and run the independent fixtures with:

```sh
clang -std=c11 -Wall -Wextra -Werror -pedantic -fsanitize=address,undefined \
  toolchain/bcc/*.c -o /tmp/bcc
python3 toolchain/tests/cconf/run.py --cc /tmp/bcc --mode E --stage a1 --stage a2
python3 toolchain/tests/cconf/run.py --cc /tmp/bcc --mode E --stage a1 --stage a2 \
  --target aarch64 --cc-arg=--target=aarch64
```

The direct lexer check uses the same flags and links
`toolchain/tests/bcc/splice_inside_token.c` with `toolchain/bcc/pp.c`.

## Hello-subset parser and semantics

`bcc -fsyntax-only` parses the located phase-4 tokens directly.
`--dump-types` adds the JSON-lines type records used by
`toolchain/tests/cconf/`. The same typed tree is available for code
generation while the translation-unit arena lives.

| Feature | N1570 clause |
| --- | --- |
| Integer and void specifiers, const-qualified types and pointer declarators | §§6.7.2, 6.7.3, 6.7.6.1 |
| File-scope function prototypes and definitions, parameter scope, linkage and redeclarations | §§6.2.1, 6.2.2, 6.2.7, 6.7.6.3, 6.9.1 |
| LP64 integer-constant types and concatenated ordinary UTF-8 strings | §§5.1.1.2, 6.4.4.1, 6.4.5 |
| Identifier and parenthesized expressions, function and array decay, lvalue conversion, prototyped calls and argument conversions | §§6.3.1.1, 6.3.2.1, 6.5.1, 6.5.2.2, 6.5.16.1 |
| Compound, null, expression and return statements | §§6.8.2, 6.8.3, 6.8.6.4 |

The hello subset uses explicit prototypes, including variadic declarations
for calls, and reports the first error. Local and global objects, typedefs,
aggregate and grouped declarators, old-style functions, variadic definitions,
operators, character and floating constants, and wide strings are later
language work. Both target profiles share the frontend. ARM64 machine code
and its on-box acceptance wait for A3.

```sh
python3 toolchain/tests/bcc/hello_translation_unit_types.py /tmp/bcc
python3 toolchain/tests/cconf/run.py --cc /tmp/bcc --mode syntax --stage b1
```

## AMD64 hello code generation

`bcc -S FILE` writes bas assembly to stdout. The emitter consumes the typed
tree and handles parameterless `int` and `void` definitions, `int` constants,
and non-variadic calls with an empty argument list or a single string literal.
N1570 §§5.1.2.2.3 and 6.8.6.4 govern returns, including falling off `main`.
String bytes come from the front end under §6.4.5. Under AMD64 psABI §§3.2.1–3.2.3,
the frame saves `rbp`, the stack is aligned at each call, the string address
goes in `rdi`, and the `int` result comes back in `eax`. String addresses and
call slots use RIP-relative addressing for the small code model (§3.5.1).

For a Breenix executable, assemble the output with `bas -o FILE.o FILE.s`,
then link with `bld -T userspace/programs/linker.ld --gc-sections -o FILE FILE.o libc.a`.
Use Breenix's x86-64 C ABI archive for `libc.a`, which supplies `_start` and
`puts`. The driver orchestration and wider scalar ABI lowering are 25.5c2.
The AArch64 bcc executable currently provides the shared frontend. Emitting
AArch64 code and running the ARM64 `cc-hello` acceptance are A3.

Run the host check on x86-64, or prefix `/usr/bin/python3` with `arch -x86_64`
on an ARM64 Mac with Rosetta. Supply the host tools and the Breenix archive.

```sh
python3 toolchain/tests/bcc/return_42_exit_status.py \
  --bcc /tmp/bcc --bas /tmp/bas --bld /tmp/bld --elfcheck /tmp/elfcheck \
  --libc /path/to/breenix-x86-64/libc.a
```

The host check calls the linked `main` in isolation and checks status 42. It
also verifies both full static links with `elfcheck` and verifies that the
hello object requires libc's `puts`. Guest startup and output are measured
in 25.5d.
