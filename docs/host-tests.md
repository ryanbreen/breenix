# Host tests

Host checks need the pinned Rust toolchain with rust-src and llvm-tools-preview,
Swift, Python 3, NASM, clang/LLVM and e2fsprogs (`debugfs`, `mke2fs`, `e2fsck`).
See [SDK host prerequisites](sdk/reference.md#host-prerequisites-and-sdk-build)
for the compiler requirements. Export `CARGO_BUILD_JOBS=4`. From a fresh checkout,
materialize the pinned Rust library before preparing the real launcher executables:

```sh
export CARGO_BUILD_JOBS=4
scripts/rust-fork.sh sync
scripts/build-host-test-tools.sh
```

This builds the run inspector CLI, xtask, both retired x86 QEMU launchers and
the console SHA-512-crypt helper;
it does not boot a guest. The policy tests invoke the retired launchers only to
verify that they refuse execution. The command uses the pinned Rust toolchain,
Swift and the repository's normal native build prerequisites.

The console staging tests locate the helper in `target/host-test-tools` after
that preparation command. `CONSOLE_HASH_HELPER` can select an existing executable.
Run Python discovery at normal CPU priority:

```sh
python3 -m unittest discover -s tests -p '*_test.py'
```

A missing executable is a failing prerequisite with the preparation command in
its diagnostic. `BREENIX_RUNS_BIN` can select an already built run inspector CLI;
`BREENIX_POLICY_BIN_DIR` and `BREENIX_POLICY_XTASK` select the policy launchers.

Do not wrap discovery in `taskpolicy -b`: the launcher priority checks require
normal policy, and background policy can exhaust subprocess deadlines.

The Rust host checks are the root integration tests, xtask, and the standalone
`tools/*-host-tests` crates. Build their test executables with `cargo test
--no-run` under `scripts/build-background.sh`, then run the executables at normal
priority. When invoking root test executables directly, export
`CARGO_MANIFEST_DIR="$PWD"`, which Cargo normally supplies at runtime.
The root command uses `--release --features testing,external_test_bins
--tests`; do not also run `scripts/run-structure-tests.sh`, since Cargo includes
those same suites. The console crate also has a behavioral input executable:
`cargo run --manifest-path tools/console-host-tests/Cargo.toml --bin input`.

Run the standalone `tests/*_host.py` programs separately: discovery's
`*_test.py` pattern does not include them. The SDK Python checks use
`python3 -m unittest discover -s sdk/tests -p '*_test.py'`. The run inspector's
Swift tests require the packaged app: run
`scripts/build-background.sh make -C tools/breenix-runs app-debug` before
`swift test --package-path tools/breenix-runs`. Its import hook check is `python3 tools/breenix-runs/scripts/test-import-hook.py`.
On macOS, run compilation subprocesses under `scripts/build-background.sh`
while keeping test executables and discovery at normal priority. The
production-cleanliness host checks build their own kernel images; the root host run does not require a prior guest boot.
