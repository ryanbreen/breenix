# Gate tooling: structure-suite compile cache (#890)

One round. Adds a content-addressed compile cache to
`scripts/run-structure-tests.sh`, the wrapper `docker/qemu/lib/gate-structure-preflight.sh`
calls once per `tests/*_structure.rs` file from each of the four boot gates
(`docker/qemu/run-aarch64-boot-test-strict.sh`,
`docker/qemu/run-aarch64-prod-profile-boot-test.sh`,
`docker/qemu/run-x86-boot-tests.sh`,
`docker/qemu/run-x86-prod-profile-boot-test.sh`). Before this round the
wrapper recompiled each suite from scratch on each call with no staleness
check at all -- the cost `docs/planning/green-program/gates/GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1-2026-09-06.md`
measured (its "Isolated preflight time cost" section) as ~127s on the Mac
and ~331s on beast, for 47 suites.

## What changed

- `scripts/run-structure-tests.sh`: a cache key is the sha256 of the
  compiled suite's own source file content, `rustc --version --verbose`,
  and the exact rustc flags this script passes (`--edition=2021 --test`).
  Any of the three changing is a miss. The cache lives under
  `${BREENIX_STRUCTURE_CACHE:-${TMPDIR:-/tmp}/breenix-structure-cache}` --
  outside `target/` altogether, so the kernel-swap hazard
  `docker/qemu/lib/gate-structure-preflight.sh`'s own header documents (a
  `cargo test` in the same shell session hardlinking a fresh, wrongly
  featured kernel binary over one a gate's build step just produced,
  because both live under `target/`) does not apply -- no file this cache
  reads or writes is under `target/`. A hit requires the stored key to
  match AND the cached binary to pass a `<binary> --list` sanity check;
  a binary that fails that check (corrupt, truncated, wrong platform) is
  treated as a miss and recompiled. `BREENIX_STRUCTURE_NO_CACHE=1` is a
  true bypass: it neither reads nor writes `$BREENIX_STRUCTURE_CACHE`,
  compiling to a private, single-use temp path instead. Tests still run on
  each call, hit or miss -- only compilation is skipped on a hit. The
  `[GATE_PREFLIGHT:...]` line is printed by
  `docker/qemu/lib/gate-structure-preflight.sh`, a file this round does not
  touch; that script redirects each suite's own stdout into a per-suite
  log file, so the new `cache: hit`/`cache: miss`/`cache: bypass` lines
  this round adds do not reach that summary line.
- `tests/structure_suite_cache_structure.rs` (new): four behavioral tests
  that build a private sandbox (its own `scripts/` + `tests/` + `cache/`
  directories under `std::env::temp_dir()`), copy the real
  `scripts/run-structure-tests.sh` into the sandbox's `scripts/`, and
  invoke the sandboxed copy against a trivial fixture. Because
  `run-structure-tests.sh` derives `REPO_ROOT` from its own script path,
  not the caller's cwd, the sandboxed copy's `SOURCE` resolves inside the
  sandbox -- no file under this repository's own `tests/` is read, written,
  or mutated by this suite. Pinned:
  1. `identical_source_reuses_the_compiled_binary_on_the_second_call` --
     first call misses and compiles; second call (unchanged source) hits
     and does not recompile.
  2. `a_changed_source_byte_invalidates_the_cache_and_recompiles` -- after
     warming the cache, rewriting the fixture with one different comment
     byte forces a miss and an actual recompile.
  3. `no_cache_env_var_bypasses_a_warm_cache` -- with a warm cache present,
     `BREENIX_STRUCTURE_NO_CACHE=1` still recompiles, and the warm cache's
     own binary bytes are read back byte-identical afterward (the bypass
     was not touched by it).
  4. `a_corrupted_cached_binary_is_treated_as_a_miss_and_recompiled` --
     overwriting the cached binary's bytes with plain text (preserving its
     executable bit, so the defect is content, not mode) is detected and
     forces a recompile, not a false hit.

## Isolation defect found and fixed in this round

The first draft of `run_sandboxed()` set `Command::current_dir(sandbox)`.
On the Mac this worked because a bare `rustc`/global rustup default was
already resolvable from anywhere. On beast's `breenix-x86` Incus container,
`rustc` is a rustup shim with no global default toolchain configured --
rustup resolves which toolchain to run by walking UP FROM THE CURRENT
DIRECTORY looking for `rust-toolchain.toml`, not from the invoked script's
own path. With cwd pinned to the sandbox (which carries no such file),
every sandboxed invocation failed with `rustup could not choose a version
of rustc to run`, which the new suite correctly reported as its own
`identical_source_reuses_...`/`a_changed_source_byte_...`/
`no_cache_env_var_bypasses_...`/`a_corrupted_cached_binary_...` tests all
FAILING -- caught live on the very first beast run below
(`structure_suites=50/51`, the one red being this new suite; full log
preserved at `docs/planning/green-program/gates/890-preflight-cache-serials/beast-first-attempt-50of51.txt`).
Fix: `run_sandboxed()` now pins `current_dir(repo_root())` (this
repository's real root) instead -- `REPO_ROOT` inside the script still
resolves to the sandbox from the script's own path regardless, but rustup's
directory-walk now finds this repository's real `rust-toolchain.toml`, the
same as every real caller (all four boot gates, and a person running this
script by hand) already gets by construction, since none of them run from
inside a scratch sandbox. Reverified green on both hosts after the fix
(both timing runs below post-date it).

## Claim-lint

```
claim-lint: python3 scripts/claim-lint.py                                    -> exit 0
claim-lint: python3 scripts/claim-lint.py --commit-msg <round's commit message> -> exit 0
```

## Evidence

### Mutation: break the cache key, show the stale-recompile test go red, restore

Edited the running worktree copy of `scripts/run-structure-tests.sh` so
`CACHE_KEY` was computed from `RUSTC_VERSION` and `RUSTC_FLAGS` only,
dropping `SOURCE_HASH` -- the shape a change that forgot to key on the
compiled file's own content would take. Ran
`bash scripts/run-structure-tests.sh structure_suite_cache_structure` with
the mutated script (after clearing `${TMPDIR}/breenix-structure-cache` so
the mutation had a clean cache to work from):

```
test identical_source_reuses_the_compiled_binary_on_the_second_call ... ok
test a_changed_source_byte_invalidates_the_cache_and_recompiles ... FAILED
test a_corrupted_cached_binary_is_treated_as_a_miss_and_recompiled ... ok
test no_cache_env_var_bypasses_a_warm_cache ... ok

thread 'a_changed_source_byte_invalidates_the_cache_and_recompiles' panicked at
tests/structure_suite_cache_structure.rs:156:5:
a changed source byte must invalidate the cache, got: == structure-test
cache: hit (reusing compiled cache_fixture) ==
...
test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

Exactly the targeted test reddened (the source-byte-blind key produced a
false hit); the other three pinned tests, which do not depend on
`SOURCE_HASH` being in the key, stayed green -- the mutation is narrow, not
a blanket break. Restored `scripts/run-structure-tests.sh` from the
pre-mutation copy (byte-identical `diff` confirmed against the saved
backup), reran: `4 passed; 0 failed`.

### Timing: two consecutive gate-preflight runs, Mac

`bash -c 'source docker/qemu/lib/gate-structure-preflight.sh;
gate_structure_preflight "$PWD" "${TMPDIR:-/tmp}"'`, sourced and called
directly (per the issue's own "or the function sourced" allowance), on
this worktree's 51 `tests/*_structure.rs` files
(`ls tests/*_structure.rs | wc -l` = 51; 47 at the time
GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1 measured its baseline, 4 added by
other rounds since):

```
# MISS (cache directory removed first)
[GATE_PREFLIGHT:structure_suites=51/51:critical_path_lines=259:pinned=120]
real 466.16s user 14.22s system 332% cpu 2:24.41 total   (wall 144.4s)

# HIT (immediately after, same cache; every per-suite log under
# ${TMPDIR}/breenix_gate_structure_preflight/*.log contains "cache: hit")
[GATE_PREFLIGHT:structure_suites=51/51:critical_path_lines=259:pinned=120]
real 446.88s user 10.28s system 367% cpu 2:04.44 total   (wall 124.4s)
```

Not claimed: a large aggregate wall-clock win from this pair alone. This
Mac was concurrently running unrelated sibling `cargo`/`rustc` work from
other worktrees during both measurements (`ps aux` during the run showed
other agents' `run-structure-tests.sh` processes active), which the 332-367%
CPU utilization on a run whose own loop is strictly sequential corroborates,
so wall-clock here is contended and noisy in both directions. The isolated,
uncontended per-suite check below is the clean evidence that compilation
itself is genuinely skipped on a hit.

### Isolated per-suite measurement, uncontended (Mac)

`gate_structure_preflight_wiring_structure` (a small, four-test suite),
timed standalone via `time bash scripts/run-structure-tests.sh
gate_structure_preflight_wiring_structure` with a cleared cache, then again
immediately after:

```
MISS: == structure-test cache: miss (no cached binary ... yet) ==
      == compiling gate_structure_preflight_wiring_structure ==
      0.14s user 0.06s system 126% cpu 0.156 total

HIT:  == structure-test cache: hit (reusing compiled ...) ==
      (no "== compiling" line)
      0.02s user 0.02s system 92% cpu 0.051 total
```

Also disclosed: `context_restore_structure` (7899-line suite, 103
`#[test]` functions per its own `grep -c '^test ' <log>`) took 81.39s wall
on a cold compile-and-run and showed no reduction on a warm cache-hit
rerun in this same noisy environment -- confirmed via its own per-run log
containing `== structure-test cache: hit ==` with no `== compiling ==`
line at all, i.e. the compile step really was skipped, yet wall-clock did not
drop. For this suite, execution of its 103 tests (each doing
`fs::read_dir` source-tree scans, per its own source) -- not compilation --
is the dominant cost, and this round's cache does not and is not asked to
touch that: "tests still RUN on every call (only compilation is cached)"
(claim-lint:ok: #890, quoted verbatim from this round's own dispatching
brief, not asserted here) is the deliverable's own stated scope. Not claimed: that this round
reduces `context_restore_structure`'s own wall-clock; it does not, by
design.

### Timing: two consecutive gate-preflight runs, beast (breenix-x86 Incus container)

Own clone at `/root/breenix-890` on beast (`git log -1` there:
`d18c46f53cdba941fc57e6c108dd61af9091abaf`, 2026-09-06), `rust-fork`
symlinked to `/root/breenix/rust-fork-real`, `run-structure-tests.sh` and
`tests/structure_suite_cache_structure.rs` copied in from this branch (the
post-toolchain-fix versions). Used a private
`BREENIX_STRUCTURE_CACHE=/root/breenix-890-cache` rather than the shared
default, after observing (via `ps aux` inside the container) unrelated
concurrent `rustc` activity from a separate `/root/breenix-battery` clone
building userspace test programs on this same host during an earlier
attempt against the shared default cache directory -- a private cache
directory removes any question of that unrelated process's compiles
counting as hits in this measurement.

```
# MISS (BREENIX_STRUCTURE_CACHE dir freshly created+empty)
[GATE_PREFLIGHT:structure_suites=51/51:critical_path_lines=259:pinned=120]
real 5m45.801s user 19m13.605s sys 0m25.143s   (wall 345.8s)

# HIT (same private cache dir, immediately after; every per-suite log
# under /tmp/breenix_gate_structure_preflight_890/breenix_gate_structure_preflight/*.log
# contains "cache: hit" -- 0 of 51 do not)
[GATE_PREFLIGHT:structure_suites=51/51:critical_path_lines=259:pinned=120]
real 5m12.337s user 18m37.119s sys 0m16.009s   (wall 312.3s)
```

~33s / ~10% wall-clock reduction on this pair, both legs 51/51 green. Not
claimed: this matches the ~331s-uncached figure in
GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1's own beast measurement scaling down
dramatically -- as on the Mac, the current 51-suite mix's aggregate
wall-clock is dominated by test-execution time in at least one heavy suite,
not by the wrapper's own `rustc --test` compile step this round caches.

### aarch64 strict boot gate with a warm cache

Built `kernel-aarch64` with `--features boot_tests` in this worktree
(`cargo build --release --features boot_tests --target
aarch64-breenix-kernel.json -Z build-std=core,alloc -Z
build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`,
clean build, no warnings), then ran
`bash docker/qemu/run-aarch64-boot-test-strict.sh 1` once against the
already-warm cache from the timing runs above:

```
[GATE_PREFLIGHT:structure_suites=51/51:critical_path_lines=259:pinned=120]
Guard: kernel FP/SIMD instruction check
PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0).
Error: ext2 disk not found at .../target/ext2-aarch64.img
```

The `[GATE_PREFLIGHT:...]` line's format is unchanged from each prior run
in this document; 51 of 51 per-suite logs this invocation wrote
under `${TMPDIR}/breenix_gate_structure_preflight/*.log` contain
`cache: hit`, confirming the gate's own real call path takes the cache-hit
branch, not just the standalone harness above. The gate did not reach a
boot: this fresh worktree has no prebuilt `target/ext2-aarch64.img` (an
artifact this round never builds and is not asked to), a gap unrelated to
#890 -- not fixed here, and not claimed as fixed.

## Not claimed

- Not claimed: a large aggregate wall-clock reduction on either host from
  the two-run pairs above -- both are contended by other concurrent
  processes on shared machines (documented above per host), and on beast
  the modest ~10% reduction is real but small next to the ~331s baseline
  GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1 measured, because compile time is
  not this repository's current bottleneck for the full 51-suite battery
  (test execution time in at least `context_restore_structure` dominates
  instead).
- Not claimed: a full green aarch64 strict-gate boot in this worktree --
  it stops at a missing prebuilt ext2 disk image, unrelated to this
  round's own change and not fixed here.
- Not claimed: any change to `docker/qemu/lib/gate-structure-preflight.sh`
  -- that file is untouched; only `scripts/run-structure-tests.sh` gained
  the cache, and its own summary-line format is unaffected by construction
  (per-suite stdout is redirected to a log file by the caller).
- Not claimed: concurrent-caller locking for the cache directory itself.
  Two gate invocations racing on the exact same `BREENIX_STRUCTURE_CACHE`
  path for the exact same stem at the exact same moment could each see a
  miss and both compile+write; this round does not add file locking around
  that window, and it was not asked to.
