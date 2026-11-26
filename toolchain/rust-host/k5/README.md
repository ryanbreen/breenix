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

ARM64 still needs its own hosted compiler and twin sysroot, X2 inventory, KD
loader acceptance and K5 guest cases.
