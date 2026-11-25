# Breenix dynamic loader core

The KD u3 crate is a dependency-free Rust library for little-endian AMD64
dynamic ELFs. It borrows file bytes and caller-owned scratch space. The
interpreter will provide process mappings, file access, TLS, and control
transfer. This crate has no unsafe blocks.

## ELF input and startup

- `Object::parse` reads `PT_DYNAMIC` and `PT_GNU_RELRO` through the ELF program
  headers. Its input is an immutable file image, so dynamic virtual addresses
  are translated through file-backed `PT_LOAD` ranges. `ET_EXEC` uses bias
  `0` and `ET_DYN` uses the caller-supplied load bias.
- `Scope::load` processes the executable's immediate dependencies in
  `DT_NEEDED` order, then their dependencies breadth-first. A slash in an
  expanded needed name makes it a direct path. A bare name searches the
  requester's colon-separated `DT_RUNPATH`, then `/lib` and `/usr/lib`.
  An empty component denotes the current directory. `$ORIGIN` and
  `${ORIGIN}` expand to the directory of the requesting object's canonical
  absolute pathname. The caller owns canonicalization and file access.
  The scope coalesces dependencies with the same canonical pathname.
  Additional search policy, including privileged-exec rules, belongs to the
  interpreter.
- SysV `DT_HASH` lookups select the first eligible definition in scope.
  Local and non-preemptible references bind within their defining object.
  For eager relocation, `R_X86_64_RELATIVE` computes `B + A`,
  `R_X86_64_GLOB_DAT` and `R_X86_64_JUMP_SLOT` store `S`, and
  `R_X86_64_64` stores the low 64 bits of `S + A`. An unresolved weak
  reference uses `S = 0`. An unresolved strong reference identifies the
  requesting object, RELA record file offset, and symbol name. The callback
  receives a validated runtime address and a little-endian word to store
  into a writable load. The caller must discard pending mappings if any
  relocation fails.
- Once relocations are complete, `Scope::initializers` gives the main
  executable's `PREINIT_ARRAY`, then `INIT_ARRAY` ranges in dependency-first
  order, ending with the executable. Walk each range in ascending address
  order. Cycles skip back edges. `Object::relro` gives the biased, half-open
  byte range from `PT_GNU_RELRO`. The interpreter controls page rounding
  and applies the corresponding protections after the initializer sequence.

The input profile uses RELA and SysV hash tables. Other relocation types,
symbol versioning, `DT_INIT`, text relocations, symbolic lookup, and legacy
RPATH without RUNPATH are outside this unit. TLS relocations and runtime
entry are later KD units. ARM64 builds the shared Rust code. Its kernel exec
half, X2 relocation and TLS inventory, backend, and KD acceptance remain
pending.

ELF gABI §7 describes program loading, and §§8.4–8.6 cover dependency
search, SysV hash layout, and constructor ordering. AMD64 psABI §4.4.1
defines the relocation codes and formulas. The GNU ld manual documents
`-z relro` and the resulting program header. [ELF gABI](https://gabi.xinuos.com/elf/08-dynamic.html),
[AMD64 psABI](https://raw.githubusercontent.com/wiki/hjl-tools/x86-psabi/x86-64-psABI-1.0.pdf),
[GNU ld manual](https://sourceware.org/binutils/docs-2.45/ld.html).

## Loaded object record

`LoadedObject` is the C-compatible LP64 record shared with libc. It is 48
bytes with 8-byte alignment. The interpreter owns the record and pointed-to
storage for as long as the object is published. The libc consumer borrows
them while the object is live.

| Byte offset | Rust field | C type | Meaning |
| --- | --- | --- | --- |
| 0 | `name` | `const char *` | NUL-terminated loaded object pathname |
| 8 | `bias` | `uint64_t` | Runtime address minus ELF virtual address for the object's load segments |
| 16 | `phdr` | `const void *` | Runtime pointer to the first `Elf64_Phdr` |
| 24 | `phnum` | `size_t` | Number of program headers |
| 32 | `dynamic` | `const void *` | Runtime pointer to the `Elf64_Dyn` array, ending in `DT_NULL` |
| 40 | `tls_module_id` | `size_t` | Nonzero TLS module ID when assigned, `0` otherwise |

## Host oracle

Run `cargo test --manifest-path libs/ld-breenix/Cargo.toml --test loader_host`
at the repository root. The committed fixture script uses clang and
`ld.lld -shared` to build the DSOs. The test reads the relocation records and
symbol addresses from `llvm-readelf` and `llvm-nm`, applies the psABI
formulas, and compares the complete address-to-value map for each object.
The fixture ELFs do not execute. Set `BREENIX_TOOLCHAIN_CLANG`,
`BREENIX_TOOLCHAIN_LLD`, `BREENIX_TOOLCHAIN_READELF`, and
`BREENIX_TOOLCHAIN_NM` to use tools outside PATH.
