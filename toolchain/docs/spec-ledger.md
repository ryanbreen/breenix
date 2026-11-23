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
| [WG14 N2244](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n2244.htm) | C11 clarification request summary with C17 dispositions, version 1.13, October 2017 | `7fa6be89b5a9a8407f5a26cc18ad86cef52d2c9683094f968bf916dba07bb0d4` |
| [System V AMD64 psABI](https://raw.githubusercontent.com/wiki/hjl-tools/x86-psabi/x86-64-psABI-1.0.pdf) | AMD64 ABI Draft 1.0, 2018-01-28 | `f5c0e21542fc379090252e18ff3d083d2a5f51da8cca62b0fec41cc330a74641` |
| [ELF gABI](https://gabi.xinuos.com/elf.pdf) | Xinuos ELF Object File Format, version 4.3 draft, 2025-09-04 | `e695f547dbb989e204dde79bc4dc6cfee7de7cca9b65a10208142932a216831b` |
| [Linux Standard Base Core Specification](https://refspecs.linuxfoundation.org/LSB_5.0.0/LSB-Core-generic/LSB-Core-generic.pdf) | 5.0, Generic Part, chapter 11 (E25 ruling 8) | — |
| [Intel SDM](https://cdrdv2-public.intel.com/812392/325462-sdm-vol-1-2abcd-3abcd-4.pdf) | Combined volumes 1–4, 325462-082US, December 2023 | `3b2551f01b4c91dc80e7f9cfb1e9790feeefe503cff15d2922ace049f4d8730f` |
| [AMD APM](https://kib.kiev.ua/x86docs/AMD/AMD64/40332-r4.09.pdf) | AMD publication 40332, combined volumes 1–5, revision 4.09, March 2026, archived PDF | `ae18704f924bb80bc9bbfe9d3c33b97c16cdeaa9c7afccb033d136e86fd88a08` |
| [IEEE Std 754](https://standards.ieee.org/ieee/754/6210/) | IEEE 754-2019, 2019-07-22 (E25 ruling 44) | — |
| [AAPCS64](https://github.com/ARM-software/abi-aa/releases/download/2025Q1/aapcs64.pdf) | 2025Q1, 2025-04-07 | `11e6b27bb994b23e6a28b7ac27b1437591cfc13c2bf77262fb8f7ea2a0f254b8` |
| [ELF for the Arm 64-bit Architecture (AArch64)](https://github.com/ARM-software/abi-aa/releases/download/2025Q1/aaelf64.pdf) | 2025Q1, 2025-04-07 (E25 ruling 29) | `bb30810c55b8fe80badf897c9dc82bb2de1d042b14d35190669cd021cc1240f2` |
| [Arm Architecture Reference Manual](https://documentation-service.arm.com/static/69d7cbfc3fb3b839f93e5b1e?token=) | Arm A-profile, DDI 0487, revision M.b (E25 ruling 31) | `0df13f5972e76226f19c1105934172e71008debd73ecff566da93ccea10cbcbf` |
| [POSIX make](https://pubs.opengroup.org/onlinepubs/9799919799.2024edition/utilities/make.html) | POSIX.1-2024, Issue 8, `make` utility | `86663126116573d3b62d89a86807e205030bd2e161beeeddfbdab7c7d25d52b4` |
| [GNU make manual](https://www.gnu.org/software/make//manual/make.pdf) | Edition 0.77, GNU make 4.4.1, 2023-02-26 | `a4bc06026984382815e392159e60a4bc356cac6c5b81e5ae84fa4f58f621f4d3` |
| [GNU C extensions](https://gcc.gnu.org/onlinedocs/gcc-15.2.0/gcc/C-Extensions.html) | GCC 15.2.0 manual, GNU C extensions chapter | `4965dab10bf21e0147e86aa80fca2a92da2c099393f34d0325c913aa3be871f2` |
| [GNU CPP Diagnostics](https://gcc.gnu.org/onlinedocs/gcc-15.2.0/cpp/Diagnostics.html) | GCC 15.2.0 manual, §5, `#warning` spelling and behavior | `fbd550c2e99d404c1f8184df32389be1011514b1d95cd0e1ff0eb8e47ca6c37e` |
| [GNU C vector extensions](https://gcc.gnu.org/onlinedocs/gcc-15.2.0/gcc/Vector-Extensions.html) | GCC 15.2.0 manual, §7.8, `vector_size` typedef spelling | `bb7f699ab3937eefe69960c5a6bbe6fae9fb58c02aa534bec65caca4c57942e6` |
| [GCC Common Type Attributes](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Common-Attributes.html#Common-Type-Attributes) | GCC 16.2.0 manual, 2026-08-07, 209636 bytes. E25 ruling 37 pins only the meaning of the record type attribute `__attribute__((packed))`. | `ab4a0dcc40060e309f2d126d3bbd8a10b2d596084d32371b8642d76b33e0d33f` |
| [NASM manual](https://www.nasm.us/xdoc/3.02/nasmdoc.pdf) | NASM 3.02, reserved for 25.12 | `a4d1766c9d640663569bcf93feac0657732fe52017002f1dbc797353f6aed34f` |
| [GNU ld manual](https://sourceware.org/binutils/docs-2.45/ld.html) | GNU Binutils 2.45, command-line options and linker script syntax | `f219c3b05e197308c5b60fc2d4dbfc302617389ca5c21501de58a90b3d052e06` |
| [Oracle Solaris `ar(3HEAD)`](https://docs.oracle.com/cd/E86824_01/html/E54772/ar.h-3head.html) | Oracle Solaris 11.3, archive file format | — |
| [Clang Language Extensions](https://releases.llvm.org/22.1.0/tools/clang/docs/LanguageExtensions.html) | Clang 22.1 release manual, builtin varargs, C11 atomics, `offsetof` and floating rounding | `66bb3c0e28f15608750483506cb079d421f9cfb5e01d25b838d921f7efabf860` |
| [LLVM Atomic Instructions and Concurrency Guide](https://releases.llvm.org/22.1.0/docs/Atomics.html#libcalls-atomic) | LLVM 22.1.0, “Libcalls: `__atomic_*`”, generic size-first runtime signatures (E25 ruling 43) | `b41cc8044842f85aeaa59feff9377a271fae5a80110c1f06ad8082183bb31bd7` |

## AArch64 assembler dialect

AAELF64 2025Q1 §§5.7.5–5.7.8 and 5.7.11.4 define the relocation codes and
relocated fields. Ruling 31 specifies bas's `:lo12:` load/store spelling and
`:tprel_hi12:` and `:tprel_lo12_nc:` spellings for the local-exec TLS ADD pair.

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

## bcc 25.5c2

| Feature | Specification |
| --- | --- |
| Scalar expressions, promotions and conversions | N1570 §§6.3, 6.4.4 and 6.5. The compiler rounds float and double constants to their target representations with integer arithmetic. |
| Block scope, declarators, initializers and control flow | N1570 §§6.2.1, 6.7 and 6.8. |
| Scalar call and return locations | AMD64 psABI §§3.1.2, 3.2.2 and 3.2.3. Integer and SSE register banks exhaust independently. Stack arguments follow source order, with padding at the high end of the argument area. |
| Variadic calls | N1570 §6.5.2.2 and AMD64 psABI §3.2.3. Trailing arguments use default promotions. The caller puts the vector-register count in `%al`. |
| Static library search | GNU ld 2.45 §2.1 for `-L` and `-l`. bcc resolves archive names for bld and preserves their order among the link inputs. |

bcc emits AMD64 and AArch64 assembly for the 25.5 scalar subset. Variadic function
definitions and `va_list` operations follow in 25.7b2, and x87 `long double`
lowering in 25.7b3. AArch64 aggregates, variadics, binary128 and C TLS follow in A4.

## bcc 25.7 record objects

| Feature | Specification |
| --- | --- |
| Record identity, tag scope and completion | N1570 §§6.2.1, 6.2.3 and 6.7.2.3. |
| Member offsets, alignment and tail padding | AMD64 psABI §3.1.2 and AAPCS64 §§5.10.1–5.10.2. |
| Member access, addresses, record assignment and initialization | N1570 §§6.5.2.3, 6.5.3.2, 6.5.16.1 and 6.7.9. The AMD64 backend copies the record's object representation. |
| By-value record arguments and results | AMD64 psABI §3.2.3 and N1570 §§6.5.2.2 and 6.8.6.4. Classify record, union and array fields recursively, allocate INTEGER and SSE registers together, and pass MEMORY records in source order. MEMORY results use a hidden first argument returned in RAX. |

For AMD64 bit-fields, N1570 §§6.3.1.1, 6.5.3, 6.5.16 and 6.7.2.1 govern
promotions, non-addressability, stores and width constraints. The AMD64
psABI §3.1.2 places the low-order bits first, keeps ordinary fields within
a unit of their declared type and excludes unnamed fields from the record's
alignment. Bit-field stores preserve adjacent bits.

The GCC 16.2 Common Type Attributes pin defines `packed` on a record
definition. Packing preserves nested types' internal layouts and the
zero-width separator's alignment rule. The AMD64 psABI §3.2.3 classifies
a packed aggregate with an unaligned field as MEMORY.

AAPCS64 §§5.5, 5.10.5, 6.8.2 and 6.9 cover AArch64 short vectors,
homogeneous aggregates, composite arguments and results. The AArch64
`vector_size` typedef subset supports 8- and 16-byte float and double
vectors as opaque values for assignment and ABI transfers. SIMD arithmetic,
lane operations and intrinsics are outside this subset.

## bmake 25.6a2

| Feature | Specification |
| --- | --- |
| Delayed expansion, substitution and assignment precedence | POSIX.1-2024 make, “Macros” and “Environment Variables”. Rule headers expand as read. Recipes expand when run. The `-e` option gives environment definitions precedence over makefile definitions. |
| Automatic variables | POSIX.1-2024 make, “Internal Macros” and “Inference Rules”. The runner uses the inferred source and stem where applicable and compares prerequisite and target timestamps for `$?`. |
| Includes and makefile selection | POSIX.1-2024 make, “Include Lines”, “Options” and “Extended Description”. Include paths resolve from the working directory and are read in order. The GNU make manual's “Summary of Options” specifies `-C`. |
| Graph traversal and freshness | POSIX.1-2024 make, “Extended Description”, “Target Rules” and “Exit Status”. Dependencies are updated in makefile order. Only a strictly newer prerequisite makes an existing file stale. |
| Suffix inference | POSIX.1-2024 make, “Inference Rules” and “Default Rules”. `.SUFFIXES` appends to or clears the ordered search list. The built-in rules cover `.c`, `.sh`, `.c.o`, `.y.o`, `.l.o`, `.y.c` and `.l.c`. The `.c.a` rule belongs to archive-member inference in 25.6c. XSI/SCCS rules are outside the serial subset. Breenix adds `.s.o` with overrideable `AS` and `ASFLAGS`. |
| Special targets | POSIX.1-2024 make, “Target Rules”. `.PHONY` opts targets into a rebuild on each invocation, `.DEFAULT` supplies a fallback recipe, and `.IGNORE` and `.SILENT` scope recipe error handling and echo. `.POSIX` declares the serial semantics bmake uses by default. `.PRECIOUS` preserves output when a recipe shell reports an interruption. |
| Recipe options | POSIX.1-2024 make, “Options”, “Makefile Execution” and “Exit Status”. `-n` prints pending recipes, with the `+` prefix still forcing execution. `-k` skips dependents of failures and attempts independent work before returning 2. `-i` continues after command errors and `-s` suppresses recipe echo. |

The serial runner can observe a signal reported in a recipe's wait status.
Under E25 ruling 31, 25.6 defers cleanup when a signal stops bmake itself, as
described in “Asynchronous Events”. A partly written non-precious target can
be left with a fresh modification time, so a later invocation can treat it as
up to date.

Ordinary makefiles default to `CC=cc` and empty `CFLAGS` so the built-in C
rules use the compiler shipped in the bundle. A leading `.POSIX:` selects
the standard compiler defaults (`c17` and `-O 1`). On Breenix, makefiles that
declare `.POSIX:` can set `CC=cc` and `CFLAGS=` to use the bundled compiler's
current language and optimization subset.

25.6c covers archive members and their timestamps, remaking include files,
recursive `MAKEFLAGS` and `$(MAKE)`, assignment operators `::=`, `:::=`, `+=`,
`?=` and `!=`, the `.WAIT` barrier, `.NOTPARALLEL`, the `-j` job pool, and
target cleanup when a signal stops bmake itself.

### AMD64 variadic definitions (25.7b2)

N1570 §7.16.1 governs the four `stdarg.h` builtins. AMD64 psABI §3.5.7
Figure 3.34 gives the one-element array type and its 24-byte record.
The prologue uses the save offsets in Figure 3.33 and guards XMM spills
with AL. The usable SSE bank ends at offset 176 because §3.2.3 assigns
only XMM0–7 to arguments. Register-class selection and whole-record
overflow follow §3.2.3.

On AArch64, AAPCS64 2025Q1 §10.1.5 Table 6 defines the five-field
`va_list` structure and §14 defines the register save areas and cursors.
Named arguments advance the cursors using §6.8.2. Anonymous arguments use
the same classification, including a pointer slot for a large ordinary
composite under B.4. Register-resident homogeneous composites are gathered
from separate 16-byte slots.

### AMD64 extended precision (25.7b3)

AMD64 psABI §§3.1.2 and 3.2.3 assign `long double` 16-byte size and
alignment, X87/X87UP stack arguments and an ST(0) result. Section 3.5.7
aligns variadic arguments in the overflow area. Only the ten value bytes
are significant. An aggregate follows the same eightbyte merging rules.

N1570 §§6.3.1, 6.4.4.2 and 6.5 govern conversions, constants and the
usual arithmetic conversions. The literal parser rounds the exact ratio
to nearest with ties to even, including the 64th significand bit and the
subnormal range. Intel SDM Vol. 1 Table 4-3 and Figure 8-13 define the
value encoding. The instruction forms come from Vol. 2. Integer
conversion truncates with a temporary x87 control word and restores
the saved control word.

### AArch64 binary128 literals, ABI and runtime (A4)

IEEE 754-2019 §3.4 and Table 3.5 define the binary128 encoding. Sections
4.3, 5.5.1, 5.11, 6 and 7 govern rounding, negation, comparison and special
results. bcc rounds literals from their exact integer ratios under N1570
§6.4.4.2. Static binary128 objects use 16-byte-aligned data or zero-filled
storage, with the literal and cast forms supported by constant initialization.
Negation flips the sign bit.

AAPCS64 2025Q1 §§5.5, 5.10.5, 6.8.2 (C.1–C.4), 6.9 and 14 place
binary128 arguments, results and homogeneous aggregate members in Q registers.
Stack arguments align to 16 bytes, and variadic arguments consume full
16-byte slots in the SIMD/FP save area before using the stack.
Arm DDI 0487 M.b §§C7.2.230 and C7.2.387 describe the Q load/store forms.
Section C7.2.157 defines the FMOV transfers between the SIMD register's
halves and the integer registers used at the brt call boundary.

The runtime uses roundTiesToEven and returns the positive quiet NaN with
its payload bits clear for invalid results and NaN inputs. It does not update
floating-point exception flags.

With the `exports` feature, `libbrt.a` provides `__brt_tf_add`,
`__brt_tf_sub`, `__brt_tf_mul`, `__brt_tf_div` and `__brt_tf_cmp`. Each
takes two unsigned 128-bit integers and returns one. AAPCS64 2025Q1
§§5.1, 6.8.2 (C.10–C.11) and 6.9 place the inputs in x0–x1 and x2–x3,
and the result in x0–x1. Compare returns the unsigned encodings of −1,
0, 1 and 2 for less, equal, greater and unordered, respectively.

IEEE 754-2019 §§5.4, 5.8, 6.2 and 7 define the conversion results. The
32-bit and 64-bit signed and unsigned integers and finite binary32 and
binary64 values widen exactly. Narrowing to binary32 or binary64 uses
roundTiesToEven, including subnormal results and overflow to infinity.
Floating-point conversions preserve the sign and quiet NaNs, keeping the
low payload bits that fit. Integer results use roundTowardZero and
return `0` for NaN, infinity or an unrepresentable truncated value
(N1570 §6.3.1.4).

With `exports`, `__brt_tf_from_*` and `__brt_tf_to_*` use x0–x1 for the
binary128 side and w0 or x0 for the 32-bit or 64-bit side. The `f32` and
`f64` symbols take and return the binary encoding as an unsigned integer.

### Floating limits and environment (25.9b)

N1570 §§5.2.4.2.2, 6.3.1.4–5 and 6.4.4.2 define the limits,
conversions and literals. The preprocessor and constant evaluator use the
same target format. They compute the limits and round literals using
integer arithmetic, with x87 extended precision on AMD64 and binary128
on AArch64. `FLT_EVAL_METHOD` is 0.

N1570 §7.6 and Annex F describe floating-state access. bcc accepts
`#pragma STDC FENV_ACCESS` and uses conservative ON behavior throughout
the translation unit. Arithmetic and conversions in executable code use
the live environment, including expressions with only literal operands.
The pinned Clang Language Extensions document the spelling and return
values of `__builtin_flt_rounds()`. On AMD64 the builtin uses
`STMXCSR` and maps RC bits 14:13 to the C values. Intel SDM Vol. 1
§§4.8.4, 8.1.5 and 10.2.3.2 specify the x87 and SSE rounding controls.
The AArch64 backend still needs to lower the rounding query to FPCR.

### AMD64 scalar atomics (25.9c1)

N1570 §§6.2.6.1p9, 6.3.2.1, 6.5.2.4, 6.5.16, 6.7.2.4, 6.7.3
and 7.17 define atomic types, value conversion and operations. The pinned
Clang Language Extensions define the `__c11_atomic_*` interface.
N1570 §§6.5.17 and 6.7.4 cover the comma expression and internal-linkage
inline functions that the existing `stdatomic.h` uses.

Intel SDM Vol. 3A §§9.1.1–9.1.2 and 9.2.2–9.2.3 in the pinned
December 2023 edition specify aligned scalar access and memory ordering.
bcc uses MOV for atomic loads and relaxed or release stores, memory
XCHG for sequentially consistent stores and exchanges, LOCK XADD or
LOCK CMPXCHG for read-modify-writes, and MFENCE for sequentially
consistent thread fences. Signal fences and other thread fences rely
on bcc preserving the order of emitted accesses. Floating compound
operations use Breenix's `fenv.h` functions to retain the floating-point
exceptions of the successful compare-exchange attempt.

AArch64 atomic instruction selection and ordering await the AArch64
backend.

### AMD64 aggregate and under-aligned atomics (25.9c2)

The generic ABI follows the pinned LLVM Atomic Instructions and Concurrency
Guide, “Libcalls: __atomic_*”. bcc passes the object size and address,
separate value or result buffers, and the supplied memory orders. A
compare-exchange passes the caller's expected pointer and can implement
the weak form with the same strong helper. Natural alignment is required
for inline operations on objects of size 1, 2, 4 or 8. bcc checks an
address at runtime when it cannot establish that alignment. The Clang
size-only builtin reports support for naturally aligned objects of the
given size. bcc's implementation of the C11 `atomic_is_lock_free` macro
also checks the address so that packed members report the selected path.

### AMD64 local-exec thread storage (25.7c)

N1570 §§6.2.2, 6.2.4, 6.7.1, 6.7.9 and 6.9.2 govern linkage, storage
duration, declarations, initialization and tentative definitions. ELF
gABI §§3.10, 5.2 and 7.7 describe `SHF_TLS`, `STT_TLS` and the
`PT_TLS` template. AMD64 psABI §10.3.2 Tables 10.7 and 10.10 give the
local-exec address and value sequences, using Breenix's self-pointer at
`FS:0` and `R_X86_64_TPOFF32` relocations. A4 implements AArch64 local-exec
TLS using TPIDR_EL0 and the AAELF64 TLS relocations and stages its suite inputs.

### C17 mode and warnings (25.10a1)

`-std=c17` defines `__STDC_VERSION__` as `201710L` (ISO C17 §6.10.8.1).
`-std=c11` and the default define it as `201112L` (N1570 §6.10.8.1).

| WG14 N2244 report | bcc behavior |
| --- | --- |
| DR 412, §6.10.1p6 | After a taken conditional, skip the remaining `#elif` expressions. |
| DR 423, §§6.5.4p5 and 6.7.6.3p5 | In C17 mode, discard top-level qualifiers from cast types and function result types. |
| DR 444, §§6.7.2.1 and 6.7.5 | Existing support for alignment specifiers on members. |
| DR 465, §7.17.5.1 | On AMD64 in C17 mode, evaluate the pointer argument once and determine the lock-free result from its pointed-to type, including for a null pointer. AArch64 atomic lowering remains ARM 25.9d work. |
| DR 481, §6.5.1.1p2 | Use lvalue, array-to-pointer and function-to-pointer conversions when selecting a generic association. The controlling expression stays unevaluated. |

`#warning` emits a located diagnostic. `-Wall` also enables warnings
for unrecognized source pragmas under §6.10.6. `-Werror` promotes bcc
warnings to errors and `-w` suppresses warnings, even with `-Werror`.
`-fno-builtin` is accepted because bcc already compiles ordinary library
names as calls. Explicit compiler intrinsics remain available.
