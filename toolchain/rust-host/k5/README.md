# K5 Rust compiler twins

E25 plan §5.1 and ruling 111 use one bootstrap from the hosted Rust pin in
`rust-fork.toml`. The compiler hosts are `aarch64-apple-darwin` and
`x86_64-breenix-hosted`. The target sysroot serves `x86_64-breenix`, the
soft-float tools target, and `x86_64-breenix-hosted`, the hard-float target
for the compiler and the later unwind case.

Use the K4 LLVM builds from the fork's LLVM gitlink and the X2/KD linker and
Cargo patch configuration. Set the bootstrap host and target lists as above,
point `build.build-dir` into the K5 cache, and set `build.jobs = 4` and
`rust.omit-git-hash = false`. The fork's library workspace uses Breenix's
`libs/libc` adjacent to the fork checkout. Keep Cargo offline with the retained
build dependencies.

For the KD linker, set `BREENIX_X2_RUNTIME` to the directory containing
`dynamic-libs/` and `cxx-pic/`. Set `BREENIX_X2_LLD` to the pinned
toolchain's `bin/gcc-ld/ld.lld` so the wrapper can find its companion LLVM
tools in `bin/`. The first attempt reached the driver link with an incorrect
runtime path. The successful invocation reused that attempt's compiled
objects. Both exit receipts and log digests are in `x86_64.json`.

From the pinned fork, run the following command through a tracked build
process with the K5 budget. Here `cache` is the run directory under
`~/Library/Caches/breenix-e25/k5`.

```sh
export CARGO_BUILD_JOBS=4 CARGO_NET_OFFLINE=true
git status --porcelain --ignore-submodules=none -- compiler src/llvm-project > "$cache/o5-before"
taskpolicy -b nice -n 15 python3 x.py build --stage 1 compiler/rustc library/std \
    --config "$cache/bootstrap.toml" --verbose
```

Keep the output in `bootstrap.log` and the runner's timing and exit receipt in
`bootstrap.json`. Run `scripts/rust-host/rustc_port.py --fork <fork> --cache
<cache>` from Breenix to capture the post-build O5 status, stage the target
rlibs from the Mac compiler's stage-1 sysroot and write the build and corpus
records. `x86_64.json` lists the staged paths and their SHA-256 digests.

The test `RustcPort.test_twin_rlibs_match_twin_compiler` asks the Mac twin to
decode the metadata of each staged rlib. Rust's metadata banner includes the
compiler release and abbreviated Git hash. The test ties that banner to the
full hash from `rustc -vV` and verifies the same release, hash and date in the
driver referenced by the x86-64 hosted ELF. The guest identity case will
compare the hosted compiler's live `-vV`. For the negative control, set
`BREENIX_K5_SYSROOT` to a sysroot populated by the pinned nightly. The metadata
dump reports the compiler version mismatch.

`corpus/expect.json` records the Mac twin's four `-vV` identity lines, the
expected hosted target name, the SHA-256 of `hello.o` for the tools target,
and the compiler's primary type-error location. Invoke the compiler from the
corpus directory with relative input names, `--edition=2021 --emit=obj
-Dwarnings`, the selected target and the staged sysroot. Add `--crate-type=lib`
for `hello.rs` and `--error-format=json` for `type-error.rs`. Compile
`uses-std.rs` as a binary crate to check that its target std resolves.

The bootstrap logs include the retained dependency warnings, unused macros in
the pinned compiler builtins, and std dropping its dylib crate type for the
soft-float target. The corpus compiles with `-Dwarnings`.

## AArch64 bootstrap and X2

Use a private checkout at the same hosted pin. The hosts are
`aarch64-apple-darwin` and `aarch64-breenix-hosted`. The targets are
`aarch64-breenix` and `aarch64-breenix-hosted`. Keep the bootstrap
configuration, Cargo patch copies, build directory and receipts under
`~/Library/Caches/breenix-e25/ak5`. Check that `compiler/` and
`src/llvm-project` are clean before and after the invocation above.

Select the Mac-native LLVM from the pinned gitlink and the K4 AArch64
archives through their respective `llvm-config` executables. If the K4 cache
retains only the archives, regenerate the K4 build graph's header outputs
under `include/llvm`, including `CodeGen/GenVT.inc`. Point the AArch64
`llvm-config` include flags at the regenerated headers. For the
driver, build separate PIC archives with K3's CMake configuration and
`CMAKE_POSITION_INDEPENDENT_CODE=ON`. Copy the approved Breenix locale
header into the generated libc++ includes before building those archives.
Apply the existing Breenix dependency patches through a build-local Cargo
config and lockfile, as in the x86-64 route. Build an ARM hosted sysroot with
the pinned stage-0 Cargo and `-Zbuild-std=std,panic_unwind`. Add its rlib
directory to the hosted compiler build's Rust flags with `-Lall=`, and set
`X2_SYSROOT_DEPS` to that directory for the getrandom provider. Scope
`-Crelocation-model=pic` to Cargo builds for `aarch64-breenix-hosted` so the
bootstrap can link the driver as a shared object. The target JSON's only
dynamic-linking addition is `dynamic-linking` (ruling 153).
For C++ compiler probes that link an executable, select the pinned ELF
LLD with the ordinary ARM `libc.a` and `userspace/programs/linker.ld`.

Configure the hosted linker as `scripts/rust-host/kd_link.py`, with
`BREENIX_X2_ARCH=aarch64` and `BREENIX_KD_BUILD_DIR` naming u3's
`libc.so`, CRT and DSO handle. The ARM `--kd-libc` build retains u3's
archive target settings and links the shared object with LLD. Point
`BREENIX_X2_BUNWIND` to a PIC `libbunwind.a` built for the hosted ARM target.
The fallback linker for intermediate shared objects includes u3's shared
libc and the PIC bunwind archive with its panic handler localized by
`kd_link.prepare_bunwind`. Use `BREENIX_X2_RUNTIME`
for the K3 archive directory in the layout described above and
`BREENIX_X2_STAGE` for the X2 link maps.

[`x2-aarch64.json`](../x2-aarch64.json) records the twin sysroot and ELF
digests, runtime imports and PT_LOAD frame count.

The ARM64 launcher stages the compiler, KD runtime and suite fixtures from
these records. ARM64 still needs guest acceptance for the dynamic and rustc
suites, followed by K6 (Rust linking), K7 (Cargo) and A5 (self-hosting).
