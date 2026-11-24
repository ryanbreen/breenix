# K4 LLVM cross build

The records in this directory describe the pinned rust-fork gitlink and the native
TableGen build followed by each Breenix cross configure. The target list comes from
`bootstrap.example.toml`'s documented `llvm.targets`. LLVM and its C++ headers are
opaque workload inputs. The cross compiler uses the localized K3 libc++ headers.
`llvm_port.py --compile` reuses the configured architecture directory and
selects the static LLVM archives and the `llc` and `llvm-mc` object targets.

`<llvm-project>` names the fork's `src/llvm-project` checkout, `<repo>` this
checkout, `<scratch>` the corresponding architecture directory under
`<cache>/k4`, and `<home>` the invoking user's home. Build directories and logs
are retained under `~/Library/Caches/breenix-e25/k4/`; copies of the logs and
records are retained in `fx/evidence/e25-k4-u1-current/`. The native build has a
1,800-second budget and uses at most four jobs. Both cross configurations use
its native tools. The per-stage diagnostics and exact allowlists include the
host linker probe, host archiver warning, and CMake warnings; they remain in the
original logs. The default timeout for earlier callers remains 55 seconds.

The x86-64 build record stops at LLVM's `GetMainExecutable` platform error.
A targeted compile reproduces it on ARM64, whose record covers configuration.
The archive inventory, executable links, and guest cases remain pending.

## Ordinary userspace parity

Ruling 98's comparison against `1ee08df8` matched 229 of 231 x86-64
executables and 230 of 230 ARM64 executables. The x86-64 differences are:

- `suite-ipc` changed `.text`, `.rodata` and `.got` bytes, the layout
  of `.text`, `.eh_frame` and `.eh_frame_hdr`, the first `PT_LOAD`
  and the entry point.
- `suite-rust-std` changed `.text`, `.rodata` and the entry point.

K3's `strftime_l` export regrouped libc codegen units, moving `_start`
and `realpath`. The control in `fx/evidence/e25-k4-u2-r95-parity.Oh3dwkIZ/`
omitted only that export and restored the allocated bytes, layouts,
program headers and entry points. K4's later x86 acceptance includes
`ipc` and `rust-std` alongside `llvm` and `cxx-runtime`. ARM64 still
needs its hosted LLVM executable links and guest cases.
