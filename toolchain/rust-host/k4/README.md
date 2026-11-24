# K4 LLVM cross configure

The records in this directory describe the pinned rust-fork gitlink and the native
TableGen build followed by each Breenix cross configure. The target list comes from
`bootstrap.example.toml`'s documented `llvm.targets`. LLVM and its C++ headers are
opaque workload inputs. The cross compiler uses the configured K3 libc++ headers.
No target LLVM library or executable is built in this unit.

`<llvm-project>` names the fork's `src/llvm-project` checkout, `<repo>` this
checkout, `<scratch>` the corresponding architecture directory under
`<cache>/k4`, and `<home>` the invoking user's home. Build directories and logs
are retained under `~/Library/Caches/breenix-e25/k4/`; copies of the logs and
records are retained in `fx/evidence/e25-k4-u1-current/`. The native build has a
1,800-second budget and uses at most four jobs. Both cross configurations use
its native tools. The per-stage diagnostics and exact allowlists include the
host linker probe, host archiver warning, and CMake warnings; they remain in the
original logs. The default timeout for earlier callers remains 55 seconds.

ARM64 has a cross-configured build graph here; its LLVM libraries, executables,
and guest cases remain for later K4 units and the ARM catch-up.
