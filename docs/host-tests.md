# Host tests

The Python launcher tests exercise real host executables. From a fresh checkout,
prepare the launcher executables with:

```sh
scripts/build-host-test-tools.sh
```

This builds the run inspector CLI, xtask and both retired x86 QEMU launchers;
it does not boot a guest. The policy tests invoke the retired launchers only to
verify that they refuse execution. The command uses the pinned Rust toolchain,
Swift and the repository's normal native build prerequisites.

The console staging tests also require the SHA-512-crypt helper. Build it at
background priority on macOS, then run Python discovery at normal CPU priority:

```sh
taskpolicy -b rustc --edition=2021 -O scripts/metal/sha512-crypt.rs -o /tmp/sha512-crypt
export CONSOLE_HASH_HELPER=/tmp/sha512-crypt
python3 -m unittest discover -s tests -p '*_test.py'
```

A missing executable is a failing prerequisite with the preparation command in
its diagnostic. `BREENIX_RUNS_BIN` can select an already built run inspector CLI;
`BREENIX_POLICY_BIN_DIR` and `BREENIX_POLICY_XTASK` select the policy launchers.

Do not wrap discovery in `taskpolicy -b`: the launcher priority checks require
normal policy, and background policy can exhaust subprocess deadlines.
