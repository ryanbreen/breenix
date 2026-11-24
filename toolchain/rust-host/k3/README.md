# K3 C++ runtime build

`cxx_runtime.py` checks the LLVM checkout against the Rust fork's pinned
`src/llvm-project` gitlink and refuses a dirty workload. It runs the documented
runtimes CMake build for either architecture with the Breenix libc headers and
records the configure result and the archive hashes or first build error.
The build directory is disposable; successful archives retain their respective
`LICENSE.TXT` notices beside them. Provide a clean pinned checkout explicitly:

```sh
python3 scripts/rust-host/cxx_runtime.py x86_64 --llvm-project /path/to/llvm-project --scratch /tmp/k3-x86 --record toolchain/rust-host/k3/x86_64.json
python3 scripts/rust-host/cxx_runtime.py aarch64 --llvm-project /path/to/llvm-project --scratch /tmp/k3-arm --record toolchain/rust-host/k3/aarch64.json
```

The symbol-closure check is an intentionally unresolved K3 gate. Set
`BREENIX_CXX_RUNTIME_ROOT` to a directory with one subdirectory per architecture,
each containing `lib/libc++.a`, `lib/libc++abi.a`, the hosted C-ABI `lib/libc.a`
and the pinned build's `lib/compiler_builtins.rlib`. It uses the pinned Rust
LLVM's `llvm-nm`, checks both architectures and rejects unwinder references.
A failed build does not claim that its missing archives pass that check.

The recorded builds stop at Ninja's duplicate `libc++abi.a` rule, before
compilation. CMake's Generic platform converts a shared target to static, so
no runtime archives or adjacent notices were produced. The coordinator ledger
has the blocker and the CMake warnings; these records do not claim a build pass.
