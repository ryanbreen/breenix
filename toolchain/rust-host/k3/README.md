# K3 C++ runtime build

`cxx_runtime.py` checks the LLVM checkout against the Rust fork's pinned
`src/llvm-project` gitlink and refuses a dirty workload. It runs the documented
runtimes CMake build with the Breenix libc headers. Both CMake compiler flag
sets and `scripts/workload-cxx` request the POSIX declarations with
`_POSIX_C_SOURCE=200809L` (POSIX.1-2024 section 2.2.1). The C++ linkage guards
in `<wchar.h>` keep its C17 section 7.29 declarations compatible with the
conversions also declared in `<stdlib.h>`. The guards in `<ctype.h>` and
`<time.h>` bind calls to the C ABI symbols declared by C17 sections 7.4 and
7.27 and POSIX.1-2024.

K3 disables localization and filesystem support. The build with filesystem
support enabled reported an undeclared `copy_file_impl_fstream`.
[libc++ vendor options](https://libcxx.llvm.org/VendorDocumentation.html#vendor-configuration-options)

CMake uses Homebrew LLVM's archive tools to index the ELF members. Archives
retain their respective `LICENSE.TXT` notices beside them. On the Mac, point
the script at a clean pinned LLVM checkout and use a fresh build directory.

```sh
export BREENIX_LLVM_PROJECT=/path/to/llvm-project CARGO_BUILD_JOBS=4
export BREENIX_CXX_RUNTIME_BUILDS="$(mktemp -d)"
for arch in x86_64 aarch64; do
    run="$BREENIX_CXX_RUNTIME_BUILDS/$arch"
    python3 scripts/rust-host/cxx_runtime.py "$arch" --scratch "$run" --record "$run/record.json"
done
python3 tests/cxx_runtime_test.py \
    CxxRuntimeTest.test_archives_build_both_arches \
    CxxRuntimeTest.test_runtime_diagnostics_match_record \
    CxxRuntimeTest.test_header_changes_keep_c_objects_identical
```

The tracked records give the compiler identity, configure and build results,
archive and notice digests, and the exact warning text and counts with build
paths normalized. The diagnostic check compares the new logs to those records.
The C object check compares the corpus and userspace inputs against the
pre-unit headers. Constraint diagnostics have their own corpus fixtures.

Use the matching build's generated headers to compile a C++ workload object.

```sh
scripts/workload-cxx --arch x86_64 --runtime-build "$BREENIX_CXX_RUNTIME_BUILDS/x86_64/build" -- -std=c++23 probe.cpp -o probe.o
```

Build and inspect the three hosted C++ probes with those same runtime builds.
The executables and link maps go to `userspace/programs/target/cxx-runtime/<arch>/`.
The build uses the pinned Rust toolchain's `ld.lld` and freshly selects the
C-ABI libc build's `compiler_builtins` artifact from Cargo's output.

```sh
taskpolicy -b userspace/programs/build.sh --arch x86_64 --cxx-runtime
taskpolicy -b userspace/programs/build.sh --arch aarch64 --cxx-runtime
python3 tests/cxx_runtime_probe_test.py
```

Each probe takes the pipe's write descriptor as its sole argument. The reader
expects the exact byte stream in `userspace/programs/cxx/expect.json` and exit
status 0. The build leaves guest staging and execution to the later K3 units.

Run the hosted link check with `BREENIX_CXX_RUNTIME_BUILDS` set as above and
`BREENIX_CXX_RUNTIME_ROOT` pointing to one subdirectory per architecture,
each containing `lib/libc++.a`, `lib/libc++abi.a`, the hosted C-ABI `lib/libc.a`
and the pinned build's `lib/compiler_builtins.rlib`.

```sh
python3 tests/cxx_runtime_test.py CxxRuntimeTest.test_cxx_runtime_symbols_resolve
```

On both architectures, the archive inventory leaves exactly the two
`__lcxx_override` boundary references and rejects exception and unwinder
references. The probe links with `linker-hosted.ld` and `--gc-sections`.
`llvm-nm` must report an empty undefined set. The check uses `llvm-readelf`
to verify that `__start___lcxx_override` equals the section address and
`__stop___lcxx_override` equals the address plus its size (ruling 89).

The Generic build generated duplicate archive rules: `cxxabi_shared` and
`cxxabi_static` both claimed `libc++abi.a`; `cxx_shared` and `cxx_static`
likewise claimed `libc++.a`. The Breenix platform module gives the shared
targets distinct `.so` names. The script requests only static targets and
stages only the two named archives.

The AArch64 runtime and probe links are cross-builds. ARM image staging
and execution of the three `cxx-runtime` cases remain pending.
