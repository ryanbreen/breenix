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

The Generic build generated duplicate archive rules: `cxxabi_shared` and
`cxxabi_static` both claimed `libc++abi.a`; `cxx_shared` and `cxx_static`
likewise claimed `libc++.a`. The Breenix platform module gives the shared
targets distinct `.so` names. The script requests only static targets and
stages only the two named archives.

Both recorded configurations succeed. Both builds stop at an unknown
`pthread_mutex_t` in libc++'s pthread support. Breenix's `pthread.h` declares
that type inside its `__BREENIX_POSIX` feature gate. The records also retain
compiler and CMake warnings. No archives or adjacent notices were produced.
