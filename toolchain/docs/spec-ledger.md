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
| [GCC Common Predefined Macros](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/cpp/Common-Predefined-Macros.html) | GCC 16.2.0 manual, 2026-08-07, §3.7.2, 37380 bytes. Covers predefined type macro names and `__STRICT_ANSI__`. | `9d68defcfa4ed119d2c5f6386b37d49584d7f5354b334008bbae1a2b7ea74904` |
| [GCC Variadic Macros](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/cpp/Variadic-Macros.html) | GCC 16.2.0 manual, 2026-08-07, §3.6, 9784 bytes. | `ea8d59878d95fd0625789968fcebd3c7fecbd84ab18b4c1ed322b612dcbb3bf6` |
| [GCC Wrapper Headers](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/cpp/Wrapper-Headers.html) | GCC 16.2.0 manual, 2026-08-07, §2.7, 6361 bytes. The pin covers `#include_next`. | `2070d0294cb6d1436e610e6e59d7735e456c57dbc4610b093fbc1ced833848fb` |
| [GCC Alternate Keywords](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Alternate-Keywords.html) | GCC 16.2.0 manual, 2026-08-07, §6.12.23, 5255 bytes. The pin covers alternate keywords and `__extension__`. | `e03f023a80ef4e52463455c9434f29df78d8d75d7738dad1b538d9dec4a7e30f` |
| [GCC Statements and Declarations in Expressions](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Statement-Exprs.html) | GCC 16.2.0 manual, 2026-08-07, §6.12.1, 10580 bytes. Compound statement expressions, trailing values and control flow. | `0c2c44bd6374986edc6ece1431fd4da37db132b5a65c3546992eea13b486b942` |
| [GCC Referring to a Type with typeof](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Typeof.html) | GCC 16.2.0 manual, 2026-08-07, §6.12.5, 9961 bytes. Type-name and expression operands, including unevaluated fixed types. | `8fe4b4cbe70530b12931eda0b7dafbcb053c15cc5c0b673ea1bc7bdb4c304910` |
| [GCC Conditionals with Omitted Operands](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Conditionals.html) | GCC 16.2.0 manual, 2026-08-07, §6.12.13, 4373 bytes. Reuse of the tested value without a second evaluation. | `e1d7b3ea98256ef2512083a139054cec83884926711e22e227bfbe2217fb2672` |
| [GCC Case Ranges](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Case-Ranges.html) | GCC 16.2.0 manual, 2026-08-07, §6.12.14, 4122 bytes. Inclusive integer case ranges. | `b868b9414f530a2cf97c1459487cd5e1d8f75d7acd1e5151e5915c3301b859b9` |
| [GCC Variable Length](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Variable-Length.html) | GCC 16.2.0 manual, 2026-08-07, §6.2.1, 7074 bytes. | `b7a9571f295795787cfd7c7d014051e81b4113c46c0ddb29558d631e8c92c562` |
| [GCC Enumeration representation](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Structures-unions-enumerations-and-bit-fields-implementation.html) | GCC 16.2.0 manual, 2026-08-07, §4.10, 6493 bytes. | `d916c4119438ffdbf6a04a610231689fbc7848ad9ee1ea7cd5e3150733fbea40` |
| [GCC zero-length arrays](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Zero-Length.html) | GCC 16.2.0 manual, 2026-08-07, §6.2.2, 10326 bytes. Size, element alignment and record padding for a `[0]` bound. | `939209609ba8072cb465f22060829649ef183877360092c5a8f7b007a71c7ecd` |
| [GCC Designated Initializers](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Designated-Inits.html) | GCC 16.2.0 manual, 2026-08-07, §6.2.11, 9175 bytes. Inclusive range designators, evaluation once and override order. | `1aed4f2be02d14e8ccc71244aafa0df3eff08b325ef0c91894c75ccec4fd3d42` |
| [GCC Library Builtins](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Library-Builtins.html) | GCC 16.2.0 manual, 2026-08-07, §7.1, 42770 bytes. The pin covers explicit memory and string calls and the names of the floating classification and comparison builtins. | `3defeda60e43356ae0b0f30ea0d80c744b92c974cecc5a60a6f4eb49a46b96c1` |
| [GCC Bit Operation Builtins](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Bit-Operation-Builtins.html) | GCC 16.2.0 manual, 2026-08-07, §7.2.2, 33645 bytes. The pin covers `__builtin_popcountl`. | `d81a8fb713a88f96dd40547e171f74281b6c02794a53056eb72ce18b031a0e58` |
| [GCC Basic Asm](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Basic-Asm.html) | GCC 16.2.0 manual, 2026-08-07, §6.11.1, 11543 bytes. | `f4f6ff9d8b7f6cfa0bd8e6ead55be987d8517950bd9c8a8dbe80fbabf1719b6a` |
| [GCC Extended Asm](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Extended-Asm.html) | GCC 16.2.0 manual, 2026-08-07, §6.11.2 and its qualifier, template, operand and clobber subsections, 91255 bytes. | `e90ade05f7d8c9453f71405e83f6f478839699e6047883202d29f647ba0abd7b` |
| [GCC Simple Constraints](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Simple-Constraints.html) | GCC 16.2.0 manual, 2026-08-07, §6.11.3.1, 17168 bytes. | `224a6ff02ccb3dd24777ce2068083a6f2ac1c528d812ba3d6899ccd3459d4670` |
| [GCC Constraint Modifiers](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Modifiers.html) | GCC 16.2.0 manual, 2026-08-07, §6.11.3.3, 8925 bytes. | `8ddfc4f214da8368ff1e35257e7582bd2c7ee99d9fe2cfb0a9eb84d0a22c9489` |
| [GCC Machine Constraints](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Machine-Constraints.html) | GCC 16.2.0 manual, 2026-08-07, §6.11.3.4, AMD64 and AArch64, 89486 bytes. | `140b6125f2329ec39c5e55da950998284f0c78047328a219f5f8e7e47b1ebd72` |
| [GCC Other Builtins](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Other-Builtins.html) | GCC 16.2.0 manual, 2026-08-07, §7.12, 51552 bytes. The numeric and allocation builtins are documented in their own sections in this edition. | `92e8e5a16b1a305e2fce2b8e2c0b88cacea13231107e524cb55a628d569822f4` |
| [GCC Floating-Point Format Builtins](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Floating-Point-Format-Builtins.html) | GCC 16.2.0 manual, 2026-08-07, §7.2.1, 25130 bytes. Infinity constants, NaN tags and `__builtin_fpclassify`. | `9fdeb40c0bae754c0c4c9c3de744370ffcd7583ce450aa05ec805e252a0b7ce5` |
| [GCC Byte-Swapping Builtins](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Byte-Swapping-Builtins.html) | GCC 16.2.0 manual, 2026-08-07, §7.2.3, 5377 bytes. | `e894e257f7a741c62e6c9a85440a8841aa4a9243aba7aca6e824ee6aed033fca` |
| [GCC Stack Allocation](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Stack-Allocation.html) | GCC 16.2.0 manual, 2026-08-07, §7.3, 10257 bytes. The pin covers `__builtin_alloca`. | `ac4877e6c1263d823ada457b6a06822aee2523ef7da4c91e86308cf97db65a06` |
| [GCC Offsetof Support](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Offsetof.html) | GCC 16.2.0 manual, 2026-08-07, 4227 bytes. N1570 §7.19 supplies the C constant-expression contract. | `09bcdd2e8175185eddfb97ff5d03431bd37a29e5d88ed002578c3adeb087569d` |
| [GCC Optimize Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Optimize-Options.html) | GCC 16.2.0 manual, 2026-08-07, 216577 bytes. Covers `-frounding-math` and the listed Kbuild optimization options. | `2accdebdce3192e6efdff6c1cb87dd5ac739085ea8bd4bef96d4863e71cae0f5` |
| [GCC Preprocessor Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Preprocessor-Options.html) | GCC 16.2.0 manual, 2026-08-07, 42467 bytes, dependency and forced-include options | `1c2878dd4eb6db32e9ecfbe8c314cb698cd70e08a6bdcb2b233b22b3b8c702be` |
| [GCC C Dialect Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/C-Dialect-Options.html) | GCC 16.2.0 manual, 2026-08-07, 36403 bytes, language modes, builtin suppression and plain char | `23c71c380776e1d128564430afe08290ed4b7a4b6a56fdf1116765de76c1e3ab` |
| [GCC Warning Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Warning-Options.html) | GCC 16.2.0 manual, 2026-08-07, 290232 bytes, warning switches and negative warning options | `f2551f87050bb7f48c256347c915abd88202872e309e9479ea21892d69f893d3` |
| [GCC Code Gen Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Code-Gen-Options.html) | GCC 16.2.0 manual, 2026-08-07, 46930 bytes, unwind table options | `163e1cb6c36175ef9bac620811850bc0df34d7f0941be517647536c3c94bb253` |
| [GCC Directory Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Directory-Options.html) | GCC 16.2.0 manual, 2026-08-07, 21327 bytes, include search | `4b8877054b41d2ca1b02276f5006d8cdffb43c241deb4a1196bfc1111bdecaa5` |
| [GCC Overall Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Overall-Options.html) | GCC 16.2.0 manual, 2026-08-07, 48576 bytes, compilation stages and output paths | `0cad1ca693e3c992e0f35d890753c9e591d91a21ebeb8f8b65734a5a85b49ad0` |
| [GCC Common Attributes](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Common-Attributes.html) | GCC 16.2.0 manual, 2026-08-07, 209636 bytes. E25 ruling 37 and 25.10 cover `packed`, `aligned`, `noreturn` and `returns_twice`. 25.11b uses the function, variable and type attributes below. | `ab4a0dcc40060e309f2d126d3bbd8a10b2d596084d32371b8642d76b33e0d33f` |
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

## Relocatable links

GNU ld 2.45 §2.1 describes `-r` (`--relocatable`). bld applies its usual
archive search without an entry-point root. It concatenates compatible
sections by name in input order, honoring alignment and keeping zero-fill
sections out of the file payload (ELF gABI §§3.3 and 3.8). It keeps retained
groups separate, remaps their signatures and members, and retains undefined
symbols and common alignment and size for the next link (§§3.9 and 5.2–5.7).
Section-relative symbol values and RELA offsets move by their input
section's placement. The relocation kind and addend travel unchanged
(§6.1). A partial link uses these section rules without a linker script or
section garbage collection. Metadata with opaque table references needs
format-specific remapping and receives a located diagnostic.

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

## bcc 25.11b Kbuild driver flags

The staged command spellings are recorded by kind in
[`busybox-c-inventory.json`](busybox-c-inventory.json). bcc accepts the
listed spellings for this workload and rejects unrecognized options.

| Options | bcc behavior | GCC 16.2.0 option page |
| --- | --- | --- |
| `-c`, `-o FILE` | Compile to an object and select the output path. The inventory uses `-fsyntax-only` to replay the front end. | [Overall Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Overall-Options.html) |
| `-I DIR` | Add a header search directory. | [Directory Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Directory-Options.html#index-I) |
| `-D NAME[=VALUE]`, `-include FILE` | Process command-line macro options, then forced headers in argument order. Search the working directory first for a forced header, then the configured include paths. | [Preprocessor Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Preprocessor-Options.html#index-include) |
| `-MD`, `-MF FILE`, `-Wp,-MD,FILE` | Emit one make rule per C input with the source and resolved headers in first-seen order, including forced, transitive and system headers. By default the depfile uses the output stem, or the source basename, with suffix `.d`. An explicit `-MF` overrides the forwarded or default file. With `-E -MD` and no explicit depfile, `-o` names the depfile and preprocessed text goes to stdout. The Kbuild forwarding spelling is the sole supported `-Wp` form. | [Preprocessor Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Preprocessor-Options.html#index-MD). The workload specifies the forwarded CPP operand. |
| `-std=gnu99`, `-std=gnu11` | Last dialect option wins. GNU modes omit `__STRICT_ANSI__` and set `__STDC_VERSION__` to `199901L` or `201112L`. GNU syntax coverage grows in the construct units. | [C Dialect Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/C-Dialect-Options.html), [Common Predefined Macros](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/cpp/Common-Predefined-Macros.html) |
| `-Wall`, `-Werror` | Enable bcc's existing unknown-pragma warning and promote emitted warnings to errors, respectively. | [Warning Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Warning-Options.html) |
| `-Wshadow`, `-Wwrite-strings`, `-Wundef`, `-Wstrict-prototypes`, `-Wunused`, `-Wunused-parameter`, `-Wunused-function`, `-Wunused-value`, `-Wmissing-prototypes`, `-Wmissing-declarations`, `-Wno-format-security`, `-Wdeclaration-after-statement`, `-Wold-style-definition` | No-ops for the staged build. bcc has no corresponding optional warning passes and retains its ordinary C string literal types. | [Warning Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Warning-Options.html) |
| `-Wno-string-plus-int`, `-Wno-constant-logical-operand` | No-ops for the two warning suppressions in the Clang command records. bcc uses an explicit list for the staged build. | [Warning Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Warning-Options.html), compatibility behavior for unrecognized negative warning options |
| `-fno-builtin-strlen`, `-fno-builtin-printf` | No-ops. Ordinary calls to these library names already use external symbols. | [C Dialect Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/C-Dialect-Options.html#index-fno-builtin) |
| `-funsigned-char` | No-op for driver compatibility. Plain `char` retains bcc's target default (signed on AMD64 and unsigned on AArch64). | [C Dialect Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/C-Dialect-Options.html#index-funsigned-char) |
| `-O2` | Select bcc's existing 25.10 optimization pipeline. | [Optimize Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Optimize-Options.html) |
| `-Oz`, `-fomit-frame-pointer`, `-ffunction-sections`, `-fdata-sections`, `-falign-functions=1` | No-ops for driver compatibility. bcc keeps its selected optimization level, frame setup, section layout and function alignment. | [Optimize Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Optimize-Options.html) |
| `-fno-unwind-tables`, `-fno-asynchronous-unwind-tables` | No-ops. bcc already omits unwind tables. | [Code Gen Options](https://gcc.gnu.org/onlinedocs/gcc-16.2.0/gcc/Code-Gen-Options.html) |

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
| Suffix inference | POSIX.1-2024 make, “Inference Rules” and “Default Rules”. `.SUFFIXES` appends to or clears the ordered search list. The built-in rules cover `.c`, `.sh`, `.c.o`, `.y.o`, `.l.o`, `.y.c` and `.l.c`. The 25.6c `.c.a` rule compiles the member's `.c` source, inserts the object into the archive, then removes the standalone object. XSI/SCCS rules are outside the serial subset. Breenix adds `.s.o` with overrideable `AS` and `ASFLAGS`. |
| Special targets | POSIX.1-2024 make, “Target Rules”. `.PHONY` opts targets into a rebuild on each invocation, `.DEFAULT` supplies a fallback recipe, and `.IGNORE` and `.SILENT` scope recipe error handling and echo. `.POSIX` declares the serial semantics bmake uses by default. `.PRECIOUS` preserves output when a recipe shell reports an interruption. |
| Recipe options | POSIX.1-2024 make, “Options”, “Makefile Execution” and “Exit Status”. `-n` prints pending recipes, with the `+` prefix still forcing execution. `-k` skips dependents of failures and attempts independent work before returning 2. `-i` continues after command errors and `-s` suppresses recipe echo. |
| Kbuild source inventory | GNU make 4.4.1 manual §§3.1–3.3 (lines, comments and includes), 4.8 and 4.11 (special targets and static patterns), 5.1 and 5.4.1 (recipes and `.WAIT`), 6.2 and 6.5–6.13 (assignments and variable definition context), 7.2 (conditionals), 8.1–8.15 (functions), 10.5 (implicit patterns) and 12.2.1 (loading). `bmake --inventory -f FILE` reports source occurrences. `scripts/toolchain-kbuild-inventory.py` reads the pinned patch and config digests from the C inventory and runs a separate `-n` probe. |

Ordinary makefiles default to `CC=cc` and empty `CFLAGS` so the built-in C
rules use the compiler shipped in the bundle. A leading `.POSIX:` selects
the standard compiler defaults (`c17` and `-O 1`). On Breenix, makefiles that
declare `.POSIX:` can set `CC=cc` and `CFLAGS=` to use the bundled compiler's
current language and optimization subset.

25.6c assignment operators follow [POSIX.1-2024 make, “Macros”](https://pubs.opengroup.org/onlinepubs/9799919799.2024edition/utilities/make.html#tag_20_76_13_05).
`::=` stores an immediately expanded value without rescanning it on use.
`:::=` expands at assignment time while preserving `$$` and stores a delayed
macro. `+=` follows the existing macro's flavor or creates a delayed macro when
the name is undefined. `?=` creates a delayed macro only if the name is undefined.
`!=` expands the command at assignment time and runs it through `/bin/sh -c`.
It removes leading whitespace and one trailing newline from stdout, then replaces
remaining newlines with spaces before storing a delayed macro. A nonzero command
status produces a located diagnostic while retaining the captured value.

25.6c recursive make follows POSIX.1-2024 make's [Options, Environment Variables,
and Macros](https://pubs.opengroup.org/onlinepubs/9799919799.2024edition/utilities/make.html).
bmake reads inherited options before command-line options, and command-line
macro values take precedence over inherited values. It passes supported flags
and macro operands through `MAKEFLAGS` to recipes. A makefile assignment to
`MAKEFLAGS` sets the recipe value. A recipe that expands `MAKE` executes under
`-n`, and the child receives the dry-run flag.

[Include Lines](https://pubs.opengroup.org/onlinepubs/9799919799.2024edition/utilities/make.html#tag_20_76_13_02)
allows delayed remaking. bmake reads the rules before updating includes,
executes include recipes even under `-n`, and rereads the makefiles when an
include's contents change.

25.6c archive targets and prerequisites follow POSIX.1-2024 make's
[Libraries, Internal Macros and Default Rules](https://pubs.opengroup.org/onlinepubs/9799919799/utilities/make.html#tag_20_76_13_07).
For `lib(member.o)`, `$@` is `lib` and `$%` is `member.o`. Freshness uses the
member's `ar_date` as read by `bobj` from the common archive header. The
`ar_date` value `0` emitted by deterministic `bar` represents the Unix epoch.

25.6c parallel scheduling follows [POSIX.1-2024 make, “Options” and “Target
Rules”](https://pubs.opengroup.org/onlinepubs/9799919799.2024edition/utilities/make.html).
`.WAIT` separates groups in a target's prerequisite list. The scheduler
requests the next group only after the preceding group succeeds. Another
dependency path can request the same prerequisite independently. A standalone
`.WAIT:` has no effect. `.NOTPARALLEL:` gives the local scheduler one slot and
preserves `-j N` for recursive invocations.

The pool protocol is bmake's design. The top-level invocation creates a
private temporary directory and includes `BMAKE_JOB_POOL=N:<absolute-directory>`
as a quoted word in `MAKEFLAGS` beside `-j N`. A recursive bmake with the same
inherited job count joins that directory. An explicit command-line `-j`
selects its own limit and pool. Each invocation can run one target using its
implicit slot, lent by the invoking recipe during a synchronous sub-make.
Each additional target claims a token by creating one of the directories
numbered from 1 through N−1. A token stays claimed across successive recipe lines
for that target and is released on completion or failure. The pool owner
removes the directory on normal exit or after handled interruption. An
uncatchable termination can leave claims or the pool directory behind. POSIX leaves multiple asynchronous
sub-makes from one recipe unspecified.

For signal handling, [POSIX.1-2024 make, “Asynchronous
Events”](https://pubs.opengroup.org/onlinepubs/9799919799.2024edition/utilities/make.html)
names HUP, INT, QUIT and TERM. Under E25 ruling 31, a Rust callback records
the interruption for the scheduler. It stops launching recipes, signals and
reaps active recipe shells, reports their exit statuses, and removes changed
targets under the POSIX exceptions for dry runs, directories, `.PHONY` and
`.PRECIOUS`. Archive member interruption leaves the containing archive in
place. Recipe shells without terminal input run in their own process group
so the scheduler can signal the shell and commands in its group. Foreground
recipes retain terminal access.

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
On AArch64 the builtin reads FPCR.RMode [23:22] and maps it to the C
values; Arm DDI 0487 M.b §C5.2.8 specifies the rounding control.

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

### Front-end support for the Breenix libc headers

The predefined type macro names come from GCC Common Predefined Macros.
Their types match the Breenix LP64 header definitions, the AMD64 psABI
§3.1.2 and AAPCS64 §§5.1, 10.1. The Common Attributes pin supplies record
and member alignment and packing, and the two function annotations.
`returns_twice` is retained on the typed function for optimization.
The alternate keywords use the existing C parser rules for `inline` and
`restrict`, and `__extension__` has no effect on a supported expression.

Wide string literals use the target's `wchar_t` type and 32-bit Unicode
code units. N1570 §§6.4.5 and 6.7.9p15 specify concatenation, the terminator
and array initialization. N1570 §§6.5.3.4, 6.6 and 6.7.6.2 govern
constant array bounds using `sizeof`. N1570 §6.3.2.3p3 permits casting an
integer constant expression equal to `0` to `void *` as a null pointer constant.

The libc limits wrapper's `#include_next` follows GCC CPP Wrapper Headers
§2.7. The explicit memory builtin names follow GCC Library Builtins §7.1
and use the libc calls and prototypes from N1570 §§7.24.4.1 and 7.24.6.1.
`__builtin_popcountl` uses ordinary unsigned LP64 operations to implement
the count specified by GCC Bit Operation Builtins §7.2.2.

### Libc suite builtins (25.10)

N1570 §§7.12.3 and 7.12.14 specify the classification and quiet comparison
macros. GCC Library Builtins and Floating-Point Format Builtins supply
their explicit spellings and the order of the `fpclassify` result codes.
The compiler uses the semantic operand format before arithmetic conversions
for classification. The comparison macros check for NaNs before comparing
the saved operands. Infinity and quiet NaN initializers use target-format
bits, including the x87 explicit integer bit and AArch64 binary128.

`__builtin_alloca` uses the aligned stack in the caller, with allocation
lifetime through function return (GCC Stack Allocation, AMD64 psABI §3.2.2
and AAPCS64 §§6.4.5–6.4.6). On AArch64, SP stays 16-byte aligned while live
temporaries and outgoing arguments move below the allocation. The fixed
frame and the x19-based over-aligned area stay in place. The stack
adjustment and copy use the A64 forms in Arm DDI 0487 M.b §§C6.2.5,
C6.2.14, C6.2.213, C6.2.413 and C6.2.455.

### Rounding mode and expression grouping (25.10a2)

`-frounding-math` records that executable floating-point expressions
must respect the live rounding mode. bcc also conservatively treats
`FENV_ACCESS` as ON throughout the translation unit. The 25.10a3 passes
must consult `TranslationUnit::preserves_float_environment` before
folding or otherwise transforming executable floating-point operations,
including operations that yield an integer. Keep the grouping of the typed
expression tree regardless of the flag (N1570 §§5.1.2.3p14, 7.6.1 and F.9).

N1570 §§6.6, F.8.2, F.8.4 and F.8.5 govern translation-time expressions.
The flag leaves integer constant expressions and static or thread storage
initializers in translation-time evaluation. Static floating-point
arithmetic rounds each intermediate to its target format in the default
rounding direction.

### Typed AST optimization (25.10a3)

The driver accepts `-O0`, `-O1` and `-O2`. The last optimization flag
selects the level. `-O1` uses the default emission path. At `-O2`, bcc
propagates integer constants and reuses integer computations over automatic
locals without address-taking uses. Facts reset at control flow joins,
and common subexpressions use the original integer types and conversions
(N1570 §§6.3.1 and 6.5). The passes preserve volatile and atomic accesses,
and leave runtime floating-point operations under the rounding policy above
(N1570 §§5.1.2.3 and 7.6.1).

Unreachable statements can be dropped after a jump or return and in an
untaken branch with an integer constant condition. The optimizer retains
alternate entries through C labels and switch case labels (N1570 §§6.8.1,
6.8.4 and 6.8.6).

The host libc recipe check uses bcc, bas, bld, bnm and elfcheck built for
the host. It reads the x86 C-ABI `libc.a` from the same path as
`userspace/libc-suite/build.sh`, and uses the guest's x86 `libbrt.a` for
the compiler-generated atomic helpers. Set `BREENIX_LIBC_SUITE_BRT` if
that archive was built outside Cargo's target directory. The check uses
the category file's complete flags and its recipe's data-symbol policy.

### AMD64 scalar register allocation (25.10a4)

At `-O2`, the AMD64 emitter records scalar local accesses and integer or
pointer expression temporaries. Address-taken, volatile and atomic objects
use memory (N1570 §§6.5.3.2, 6.7.3 and 7.17). A linear scan extends local
intervals across backward edges and spills the interval ending last when
registers run out.

AMD64 psABI §§3.2.1–3.2.3 and Figure 3.4 govern the register bank. The
emitter saves live R8 and R9 values before loading outgoing arguments and
reloads them after the call. The prologue saves each allocated RBX or
R12–R14, and the epilogue restores it. R15 remains the aligned-frame
anchor.


### 25.11b GNU declaration attributes

`section`, `weak`, `visibility` and `alias` use the pinned GNU Common
Attributes page and ELF gABI §§3, 5 and 6. bcc preserves these attributes
across file-scope redeclarations. bas writes the requested section,
`STB_WEAK` and `st_other`, and defines aliases at the target's section and
value with its type and size. A reference to a weak symbol remains
relocatable so the linker can choose a stronger definition.

`may_alias` permits access through a pointer to an annotated type without
the N1570 §6.5p7 effective-type restriction. Its layout and C type
compatibility are unchanged. The optimizer limits value reuse to
non-escaping local objects and performs no type-based alias analysis.

`used` is satisfied by bcc's existing emission of unreferenced definitions.
`unused`, `format`, `deprecated` and `warn_unused_result` are recognized
declaration syntax. bcc has no corresponding use-site warnings today.
`noinline`, `always_inline`, `const`, `pure`, `malloc` and `cold` are
recognized function attributes. The current pipeline leaves calls in place
and performs no attribute-driven inlining, call elimination or cold layout.

### 25.11b GNU preprocessing and builtins

GNU CPP §3.6 specifies named variadic parameters and removal of the
comma in `, ##args` when the argument is omitted in GNU mode. An
explicitly empty argument retains the comma. The wrapper-header check
uses the existing §2.7 search order with both quoted and computed
header names. `-nostdinc` suppresses implicit bundle include directories
while preserving explicit include paths, per GCC Directory Options.

GCC Other Builtins §7.12 specifies `__builtin_expect`,
`__builtin_constant_p`, `__builtin_types_compatible_p` and
`__builtin_unreachable`. bcc keeps the value and single evaluation of
the first expect operand and does not use its hint to lay out branches.
The constant query is resolved during semantic analysis without
evaluating its operand. The compatibility result compares unqualified
types. The unreachable expression emits AMD64 UD2 (Intel SDM Vol. 2)
or AArch64 BRK #0 (Arm DDI 0487 M.b, the BRK instruction).

GCC Numeric Builtins §§7.2.2–7.2.3 specify the widths and results of
bit counts and byte swaps. Both targets use unsigned C operations to
lower dynamic counts and swaps, including arguments with side effects.
The existing offset operation uses the pinned GCC Offsetof Support
grammar. N1570 §§6.7.2.2 and 6.7.9p22 specify enum constant expressions
and completion of an array by its initializer.

### 25.11b GNU C inline assembly

The Basic and Extended Asm sections specify literal templates, the
volatile spelling, numeric operand substitution and the `memory` and
`cc` clobbers. bcc copies block-scope basic templates verbatim.
Extended templates represent scalar integer and pointer operands with
`=r`, `+r`, `r` and a numeric matching input. bcc evaluates operand
expressions and output addresses once and stages them before emitting the
template. The selected register bank is call-clobbered per AMD64 psABI
§3.2.1 and AAPCS64 §6.1.1. The x86 output uses bas's Intel-style dialect;
AArch64 uses A64 syntax. bas accepts full-line hash comments alongside
A64's hash-prefixed instruction immediates.

Both optimizers preserve asm statements and invalidate their local facts
across them. The memory clobber is a compiler barrier and emits no
processor fence. bcc reports unsupported qualifiers, constraints,
clobbers and template placeholders at the source site. File-scope asm
and asm goto are outside this subset.

### 25.11b workload replay

Under N1570 §§6.2.4 and 6.7.6.2 and GCC §6.2.1, automatic
arrays with a runtime outer bound evaluate the bound once. `sizeof`
uses the recorded byte count, and control flow restores the dynamic
stack on scope exit. The GNU modes accept these local arrays with
fixed-layout elements. Runtime inner bounds, over-alignment, and
mixing these arrays with function-lifetime stack allocation require
additional lowering.

GNU enumerators that exceed `int` retain a type wide enough for their
value on the target LP64 ABI. GNU static initializers can use the integer
address of a member relative to a null record pointer as an offset
(N1570 §6.6p10 and the same record layouts used by `__builtin_offsetof`).
The GNU assignment conversion between `void *` and function pointers
uses the shared pointer representation, as described by the [GCC
porting notes](https://gcc.gnu.org/gcc-14/porting_to.html).

The host replay uses the same libc and compiler-support include order
as the sealed reference. The GNU host dialect exposes Breenix's existing
declarations for POSIX.1-2024 XSH `popen`, `pclose` and `strdup`.

## bmake 25.11c directives

The GNU make 4.4.1 manual above is the edition 0.77 pin for the following
sections. The Kbuild inventory records literal directives and assignments
separately from its dry-run result.

| Feature | GNU make manual section |
| --- | --- |
| `-include` and `sinclude` | §§3.3 and 3.5. Both spellings use the existing include-and-remake flow and permit missing optional files. |
| Order-only prerequisites and phony targets | §§4.3, 4.6 and 10.5.3. Order-only edges participate in scheduling and appear in `$\|`. Normal edges determine freshness and populate `$?`. |
| Export and unexport | §5.7.2. The directives control the environment of recipe commands, with values expanded in the recipe's variable scope. |
| `:=`, override and multiline definitions | §§6.2.2, 6.7 and 6.8. `:=` uses the same immediate expansion as POSIX `::=`. |
| Target-specific assignments | §§6.11 and 6.13. A prerequisite takes its variable scope from the first target that requests it. `private` keeps a binding local to its target. |
| Conditional directives | §7.2. Each makefile has its own conditional stack. An enclosing inactive branch suppresses nested tests and assignments. |
