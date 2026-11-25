# Breenix dynamic loader

The loader core reads little-endian AMD64 dynamic ELFs using borrowed file
bytes and caller-owned scratch space. The interpreter binary adds the Breenix
syscall and startup code through the interpreter feature.

## ELF input and startup

- `Object::parse` reads `PT_DYNAMIC`, `PT_TLS` and `PT_GNU_RELRO` through the ELF program
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
- AMD64 local-dynamic `R_X86_64_DTPMOD64` stores the one-based module ID of
  the requesting object, counting only objects with `PT_TLS` in load order.
  After relocation and before constructors, the interpreter passes that
  ordered table of mapped templates to hosted libc. Its `__tls_get_addr`
  indexes the calling thread's DTV, allocating aligned storage, copying
  tdata, and zeroing tbss on first use. Thread exit releases the blocks
  after TLS destructors finish.
- Once relocations are complete, `Scope::initializers` gives the main
  executable's `PREINIT_ARRAY`, then `INIT_ARRAY` ranges in dependency-first
  order, ending with the executable. Walk each range in ascending address
  order. Cycles skip back edges. `Object::relro` gives the biased, half-open
  byte range from `PT_GNU_RELRO`. The interpreter applies page protections
  after relocation and before running constructors.

The input profile uses RELA and SysV hash tables. Other relocation types,
symbol versioning, `DT_INIT`, text relocations, symbolic lookup, and legacy
RPATH without RUNPATH are outside this unit. Shared loads occupy distinct
virtual pages and GNU_RELRO ends on a page boundary. Secure exec needs a
trusted library search policy. The u4 X2 inventory has only local-dynamic
TLS, in the driver. Dynamic shutdown and the dl* interfaces are later KD
work. ARM64 builds the shared Rust code. Its kernel
exec half, X2 relocation and TLS inventory, backend, and KD acceptance remain
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

Build the interpreter and hosted libc with `userspace/programs/build.sh
--kd-loader` and `userspace/programs/build.sh --kd-libc`. Run
`cargo test --manifest-path libs/ld-breenix/Cargo.toml --test loader_host`
at the repository root. When using a separate `BREENIX_KD_BUILD_DIR`, point
`BREENIX_LD_BREENIX` at its `ld-breenix.so` and `BREENIX_KD_LIBC` at its
`libc.so`. The
committed fixture script uses clang and
`ld.lld -shared` to build the DSOs. The test reads the relocation records and
symbol addresses from `llvm-readelf` and `llvm-nm`, applies the psABI
formulas, and compares the complete address-to-value map for each object.
The fixture ELFs do not execute. Set `BREENIX_TOOLCHAIN_CLANG`,
`BREENIX_TOOLCHAIN_LLD`, `BREENIX_TOOLCHAIN_READELF`, and
`BREENIX_TOOLCHAIN_NM` to use tools outside PATH.

The hosted shared libc and its separate executable CRT are built with
`userspace/programs/build.sh --kd-libc`. The recipe selects shared startup
in a build-local copy of libc and links its cleaned PIC archive with
`ld.lld -shared`. The dynamic suite's staging step
checks the bytes read back from `/lib/ld-breenix.so` and `/lib/libc.so`
and returns their digests for the image build record.
