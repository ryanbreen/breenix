# #890 re-land: structure-suite compile cache, four MAJORs repaired

<!-- claim-lint:ok: the 4 repairs are MAJOR-1 through MAJOR-4, each with its
     own section below naming its test; the red/green table in "Independent
     verification" below re-runs all 4 (4/4) and the whole-suite run (10/10)
     against this exact worktree. -->
PR #900 shipped a compile cache for `scripts/run-structure-tests.sh` and was
reverted (PR #902) after review found four MAJOR-severity defects: the cache
key omitted `#[path]`-included module content (MAJOR-1), omitted the
checkout root so two worktrees could reuse each other's compiled binary
(MAJOR-2), a zero-byte cached binary was silently treated as a passing hit
(MAJOR-3), and the compiled binary/key file were written non-atomically with
no lock, so a concurrent reader could observe a torn file (MAJOR-4). This
round re-implements the cache with the four repaired, each with a test that
fails against the reverted script and passes against the new one, verified
independently in this round (not only by the implementing agent).

<!-- claim-lint:ok: "every claim below" refers to the specific, itemized
     checks in "Independent verification" below (the 4-row red/green table,
     the 10/10 whole-suite run, the gate-script diff, and claim-lint's own
     exit code), each re-run and quoted in this session rather than taken
     from the agent's own report. -->
Work: astra (`gpt-6-astra`, `model_reasoning_effort=medium`) implemented the
repair from an outcome-first brief listing the four required designs; the
coordinating session (sonnet) then independently re-verified the claims
below by re-running the checks itself against the actual reverted script
(`ac3416fc^2`), not by trusting the agent's self-report.

## The four repairs

### MAJOR-1: cache key covers the transitive source set

<!-- claim-lint:ok: by construction -- source_set() in
     scripts/run-structure-tests.sh:146-168 is a recursive function that
     calls itself on every regex match its own grep -oE finds (line
     160-167), so "every file reached" is the function's own loop
     structure, not a sampled claim; the 2-level chain (#[path] ->
     include! -> include_str!/include_bytes!) is exercised by
     nested_include_forms_resolve_from_their_own_files_and_ignore_escaped_literals
     in tests/structure_suite_cache_structure.rs, run and passing in
     "Independent verification" below. -->
`source_set()` in `scripts/run-structure-tests.sh` recursively discovers
every file reached from the top-level `tests/<stem>.rs` via
`#[path = "..."]` (on a `mod` item), `include!(...)`, `include_str!(...)`,
and `include_bytes!(...)`, resolving each relative path relative to the
directory of the file the reference appears in (Rust's own resolution rule,
which is what makes multi-level chains resolve correctly). Scanning is
line-by-line with patterns anchored tightly enough not to match inside a
Rust string literal that merely contains similar text --
`tests/context_restore_structure.rs:7493` contains the literal bytes
`.find("#[path = \"")` as part of its own test logic, and the anchoring
(`#\[path[[:space:]]*=[[:space:]]*"[^"\]*"\]`, requiring an *unescaped*
quote immediately after `=`) does not match it -- confirmed directly:

```
$ grep -nE '#\[path[[:space:]]*=[[:space:]]*"[^"]*"\]' tests/context_restore_structure.rs
1:#[path = "shared_coreproof/mutation_leg_mask.rs"]
```

<!-- claim-lint:ok: by construction -- the MISSING sentinel and sort are
     scripts/run-structure-tests.sh:153-155 (missing-path branch) and :190
     (`| LC_ALL=C sort |` before hashing); the missing-dependency half of
     this claim is exercised by
     a_change_to_an_included_path_module_invalidates_the_cache_and_recompiles's
     final block (delete helper.rs, assert a miss + no stale run), and the
     cycle-termination half by
     nested_include_forms_resolve_from_their_own_files_and_ignore_escaped_literals,
     both run and passing below. The self-referential fixture path named in
     that test is a throwaway sandbox file the test itself creates at run
     time (tests/nested/data.txt under a temp sandbox root), not a repo
     path, so it does not resolve on disk here. -->
A reference to a file that does not exist on disk folds a
`MISSING:<resolved-path>`-prefixed sentinel into the hash input rather than
being skipped, so a file going from present to absent (or the reverse)
changes the key. Every discovered file's canonical path (via `realpath`,
falling back to `cd ... && pwd -P` when `realpath` is absent or fails on a
not-yet-existent path) is hashed, the set is sorted before hashing so
discovery order does not affect the key, and a `VISITED` set prevents
infinite recursion on a cyclic reference (stress-tested: see
`nested_include_forms_resolve_from_their_own_files_and_ignore_escaped_literals`
below, where an `include_str!` target's own text content happens to contain
a self-referential `include_str!("../nested/data.txt")` string and the scan
must terminate on it via canonicalization, not loop forever).

Test: `a_change_to_an_included_path_module_invalidates_the_cache_and_recompiles`.
Builds a fixture `#[path = "helper.rs"] mod helper;` top-level file plus a
`helper.rs` the test reads a value from, warms the cache, edits ONLY
`helper.rs` (top-level file byte-identical), and asserts the next run
reports a miss and recompiles. Also covers the missing-dependency case
(deleting `helper.rs` after a warm cache still reaches compilation as a
miss, where rustc reports the real missing input, rather than silently
reusing the stale binary).

### MAJOR-2: cache key and default cache directory are per-checkout

<!-- claim-lint:ok: by construction -- CACHE_KEY's inputs are printf'd at
     scripts/run-structure-tests.sh:192 and include $REPO_ROOT (canonical,
     via canonical_path at :170), so two different REPO_ROOT strings
     produce two different sha256 keys unless sha256 collides, which this
     doc does not claim to have ruled out and does not need to -- a
     mismatched key is treated as a miss at :219-221 regardless. Behavior
     exercised by two_checkouts_sharing_one_cache_directory_never_reuse_each_others_binary,
     run and passing below. -->
`CACHE_KEY` now includes the canonicalized `REPO_ROOT` (the same
canonicalization as MAJOR-1), so a stored key from one checkout will not
match another checkout's computed key even when their cache files land at
the same path -- a foreign binary is detected as a stale-key miss and
recompiled, not silently executed. Independently, the *default* cache
directory (used only when `BREENIX_STRUCTURE_CACHE` is unset) is now
`${TMPDIR:-/tmp}/breenix-structure-cache/<sha256 of realpath(REPO_ROOT)>`,
so two worktrees using the default do not share a directory in the common
case.

<!-- claim-lint:ok: "the harder case" isolates the key-level defense from
     the default-directory layer by forcing both checkouts onto one
     explicit cache dir; the test's own assertions (miss + own-manifest
     match, both directions) are the check, quoted verbatim in "Independent
     verification" below rather than asserted here. -->
Test: `two_checkouts_sharing_one_cache_directory_never_reuse_each_others_binary`.
Two independent sandbox checkouts with byte-identical fixture sources, both
pointed at the SAME EXPLICIT `BREENIX_STRUCTURE_CACHE` (the harder case,
isolating the key-level defense on its own): each run reports a miss (not a
false hit) and each run's own `MANIFEST=<CARGO_MANIFEST_DIR>` runtime output
matches its own checkout's path, not the other's. A second phase checks the
default-directory layer directly: two checkouts sharing one `TMPDIR` with no
override produce two distinct 64-hex-char subdirectories under
`breenix-structure-cache/`, each holding its own binary, and each then hits
on its own subsequent run.

### MAJOR-3: a zero-byte or otherwise-non-functional cached binary is a miss, not a silent pass

<!-- claim-lint:ok: reproduced live in this session against ac3416fc^2 (the
     actual PR #900 script, not a paraphrase): truncating a warm cached
     binary to 0 bytes and re-running printed "cache: hit" and exited 0
     with no "running N tests"/"test result:" line at all -- the exact
     silent-pass this paragraph describes. See the MAJOR-3 row of the
     red/green table in "Independent verification" below, where the same
     scenario is red against ac3416fc^2 and green against this round's
     script. -->
The old check ran `"${BINARY}" --list >/dev/null 2>&1` and trusted a zero
exit code. A zero-byte file is treated by both macOS and Linux `bash` as a
trivially-succeeding empty `/bin/sh` script (the standard `ENOEXEC`
fallback), so `--list` "succeeded" against it and the real run also fell
through the same empty-script path and also exited 0 -- a clean exit having
run zero tests. The new check requires three things before trusting a hit:
non-empty (`-s`), executable (`-x`), and `--list`'s OUTPUT actually
containing a `: test` line (not just an exit code) -- a zero-byte or
garbage file can exit 0 as a no-op shell script, but it cannot print a line
matching `: test`.

<!-- claim-lint:ok: quoted assertions are in
     tests/structure_suite_cache_structure.rs's
     a_zero_byte_cached_binary_is_treated_as_a_miss_and_recompiled, run and
     passing in "Independent verification" below; "the old design's own
     header comment" is scripts/run-structure-tests.sh's pre-#902-revert
     text (ac3416fc^2, lines 65-70), which asserted the corruption check
     alone was sufficient and did not mention execution count. -->
Test: `a_zero_byte_cached_binary_is_treated_as_a_miss_and_recompiled`. Warms
the cache, then separately truncates the cached binary to (a) zero bytes and
(b) a non-empty `#!/bin/sh\nexit 0\n` no-op script (both preserve the
execute bit via `fs::write`), and for each asserts: a miss is reported, the
message contains "corrupt", a recompile actually happens, AND -- the
property the old design's own header comment claimed but did not check for
-- the run's output contains `test result: ok. 1 passed`, not just an exit
code of 0.

### MAJOR-4: atomic publish + lock so concurrent compiles of one entry can't corrupt each other

<!-- claim-lint:ok: by construction -- scripts/run-structure-tests.sh:230-233
     shows the compile-to-tmp, mv the binary, then compile-to-tmp/mv the key
     in that literal order; the concurrency test below observed exactly 1
     compile from 2 concurrent invocations and a valid final binary, quoted
     in "Independent verification". -->
Compilation now targets a private temp path next to the final one
(`"${BINARY}.tmp.$$"`, `$$` = this process's PID) and `mv -f`s it into place
once fully written; the key file is published the same way, strictly AFTER
the binary's rename. A portable `mkdir`-based lock (`"${BINARY}.lock"`, no
dependency on `flock(1)` -- confirmed absent on this Mac by default, and the
brief called for a design that needs no external binary on either host) is
held from before the cache-hit re-check through the end of the run, so a
caller that waits for the lock re-validates the entry (a fresh hit is
possible if another caller just finished) before ever compiling, and two
concurrent callers on one entry cannot observe or produce a torn file.

Test: `two_concurrent_invocations_on_a_cold_cache_both_succeed_and_produce_one_valid_binary`.
A `rustc` wrapper intercepts real compiles, logs the `-o` target and sleeps
1s before delegating to the real compiler (widening the race window and
pinning that staging targets `<binary>.tmp.<pid>`, not the final path
directly), two processes are spawned back-to-back against the same cold
entry, both must exit 0, the final binary must pass the MAJOR-3 checks and
actually run, and -- stronger than the brief strictly required -- exactly
ONE compile is observed in the log (the waiter reused the winner's
published entry rather than compiling redundantly), with no lock directory
or `.tmp.*` debris left behind.

A fifth test not in the original four,
`nested_include_forms_resolve_from_their_own_files_and_ignore_escaped_literals`,
exercises MAJOR-1's recursion two levels deep (`#[path]` -> `include!` ->
`include_str!`/`include_bytes!`) and the escaped-literal false-positive
guard together.

## Independent verification (this round, not the implementing agent's report)

The claims in this section (the red/green table, the whole-suite runs, the
gate-script diff, and claim-lint's exit code) were re-run in this session
against the actual git objects, not taken from the agent's summary.

**Red-against-old, green-against-new, the four repairs (4/4)**, each run
individually via `git show ac3416fc^2:scripts/run-structure-tests.sh` swapped
into place, then restored via a backup copy (verified byte-identical to the
pre-swap `git diff --stat` afterward):

| Repair | Test | RED (old script) | GREEN (new script) |
|---|---|---|---|
| MAJOR-1 | `a_change_to_an_included_path_module_invalidates_the_cache_and_recompiles` | `FAILED. 0 passed; 1 failed` | `ok. 1 passed; 0 failed` |
| MAJOR-2 | `two_checkouts_sharing_one_cache_directory_never_reuse_each_others_binary` | `FAILED. 0 passed; 1 failed` | `ok. 1 passed; 0 failed` |
| MAJOR-3 | `a_zero_byte_cached_binary_is_treated_as_a_miss_and_recompiled` | `FAILED. 0 passed; 1 failed` | `ok. 1 passed; 0 failed` |
| MAJOR-4 | `two_concurrent_invocations_on_a_cold_cache_both_succeed_and_produce_one_valid_binary` | `FAILED. 0 passed; 1 failed` | `ok. 1 passed; 0 failed` |

**Whole suite, twice in a row, fresh cache**: `structure_suite_cache_structure`
(10 tests: the original 5 plus the 5 new ones above) --
`test result: ok. 10 passed; 0 failed` on both runs.

**`docker/qemu/lib/gate-structure-preflight.sh` byte-identity**:
`git diff --exit-code -- docker/qemu/lib/gate-structure-preflight.sh` --
empty diff, confirmed after the round's edits.

**`python3 scripts/claim-lint.py`**: exit 0 --
`claim-lint: clean (2 file(s) checked, changed hunks vs 6346f2c53817).`

## Timing (Apple Silicon Mac, this session)

Two separate measurements, reported honestly rather than only the flattering
one:

**Isolated single suite** (`teardown_structure`, the default stem, fresh
cache dir, back-to-back):

```
miss (cold, compiles): real 16.7s  (92 tests, 15.53s of which is test execution)
hit  (warm, no compile): real 15.6s
```

Compilation for this suite costs roughly 1.1-1.2s; the ~92-test execution
(`fs::read_dir` tree scans per test) dominates the suite's own wall-clock
regardless of cache state, matching the original review's MINOR-5 finding
about `context_restore_structure` -- this appears to generalize to most
suites, not just that one.

**Full preflight, 53/53 suites** (`gate_structure_preflight`, two
independent back-to-back cold/warm pairs, same cache directory reused for
the warm half of each pair):

```
pair 1: cold 99.6s   warm 108.1s   (53/53 hits confirmed via per-suite logs)
pair 2: cold 102.5s  warm 113.1s  (53/53 hits confirmed via per-suite logs)
```

<!-- claim-lint:ok: N-of-M given in-line -- 53/53 hits both pairs, confirmed
     by grepping each pair's own per-suite log directory
     (breenix_gate_structure_preflight/*.log) for "cache: hit" vs
     "cache: miss" counts, not read off the summary line alone. -->
**Not claimed**: the full-preflight measurement shows no net wall-clock win
on this Mac in this session -- the warm run was consistently *slower* by
~9-11s across two independent pairs, despite genuinely hitting on 53/53
suites (confirmed via per-suite log content, not just the summary line).
Direct measurement of the two candidate causes: a lone `--list` call on the
heaviest suite (`context_restore_structure`, 97 tests) costs ~2-9ms,
negligible summed across 53 suites; a full run of that same suite costs
52.8s wall-clock / 260s user CPU by itself -- roughly half of the entire
53-suite preflight's total time is test EXECUTION in this one suite, which
runs identically whether the binary was just compiled or reused. The 9-11s
delta between cold and warm pairs is most plausibly ordinary multi-minute
run-to-run variance (thermal/scheduling) on a Mac that had just finished
several minutes of heavy `rustc`/QEMU activity, not a real regression from
caching -- but this round did not isolate that further, so it is reported
as an open, unresolved discrepancy rather than asserted either way. The
practical implication: on this Mac, for this suite set, compilation is a
small fraction of the total preflight cost, so the correctness fixes in
this round matter more than the caching mechanism's aggregate speed; beast
(slower/more constrained rustc, per the original #889 measurement of ~331s
there vs ~127s here) is a more plausible place to see a proportionally
larger compile-time share, and was not re-measured in this round.

<!-- claim-lint:ok: this section's own PASS: 1/1 line, quoted below, is the
     resolving artifact for the heading; "zero warnings" is the literal
     `cargo build` transcript for this invocation, captured in this
     session's shell output (no other output line appeared besides the
     `Finished` line and one future-incompat notice about the forked std's
     own `core` crate, unrelated to this round's files). -->
## Live gate proof

`docker/qemu/run-aarch64-boot-test-strict.sh 1`, built fresh in this
worktree (`cargo build --release --features boot_tests --target
aarch64-breenix-kernel.json -Z build-std=core,alloc -Z
build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`,
no warnings from this repo's own crates; userspace and the ext2 test disk
built via `userspace/programs/build.sh --arch aarch64` and
`scripts/create_ext2_disk.sh --arch aarch64` against a locally symlinked
`rust-fork/` -- a gitignored, machine-shared build artifact not touched or
modified by this round; the symlink itself is gitignored and was not
committed), ran end to end with a genuinely cold default-location structure
cache:

```
[GATE_PREFLIGHT:structure_suites=53/53:critical_path_lines=260:pinned=120]
...
PASS: 1/1 boots succeeded
```

## Deviations from the brief

- The lock is held through test EXECUTION, not only through
  compile-and-rename, so two concurrent callers on the same entry run
  serially rather than only serializing the compile step. This is stricter
  than the brief's literal minimum (which only required no corruption, both
  processes exit 0) and was a deliberate choice to prevent a concurrent
  writer from ever replacing a binary a reader is mid-execution against on a
  shared explicit cache directory.
<!-- claim-lint:ok: by construction -- scripts/run-structure-tests.sh:214
     pipes "$BINARY" --list into `grep ': test' >/dev/null` (no -q), so
     grep reads to EOF instead of closing the pipe on its first match. -->
- The `--list` validity check consumes the full stdout of `--list` via
  `grep ': test'` rather than `grep -q`, to avoid the writer receiving
  `SIGPIPE` under `pipefail` when `grep -q` closes its input early on the
  first match -- `pipefail` would otherwise attribute that signal death to
  the whole pipeline's exit status even though a match was found.
- One extra test (`nested_include_forms_resolve_from_their_own_files_and_ignore_escaped_literals`)
  was added beyond the four required, covering a two-level `#[path]` ->
  `include!` -> `include_str!`/`include_bytes!` chain and a
  self-referential include target, to stress-test the recursion and
  cycle-termination logic the four required tests do not individually
  reach.
- No claim in this doc about beast (x86 Incus container) timing or
  behavior -- the commands quoted above ran on the Mac only, in this
  worktree; the original #890 issue's beast figure was not re-measured by
  this round.
