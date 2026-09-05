# Widening the host-wide aarch64 QEMU lock past docker/qemu/ (#834/R181)

## Why

#826/R181's own fix (`docker/qemu/lib/qemu-host-lock.sh`, landed via #835) was
scoped, by its own wording ("a shared helper ... sourced by each aarch64 gate
script"), to `docker/qemu/*.sh`, and its own structural ratchet
(`tests/qemu_host_lock_structure.rs`) policed only that one tree. #834
disclosed the gap that fix left: a `grep -rl qemu-system-aarch64 scripts/
docker/ run.sh` census found six more `.sh` scripts under `scripts/` with a
real, unwired `qemu-system-aarch64` launch line, 0 of them routed through the
lock. This branch closes that gap.

A fresh census run at this branch's own start (the same `grep -rl
qemu-system-aarch64 scripts/ docker/ run.sh` command #834's own issue body
names) found two more real launch sites #834's own disclosure did not name:

- `docker/qemu-aarch64/run-arm64-boot.sh` -- a Docker-wrapped launcher in a
  directory *sibling* to `docker/qemu/`. #826/R181's ratchet walked
  `docker/qemu` as its recursive root; a walk rooted at one directory does
  not descend into a sibling directory regardless of how its
  `launches`/`sources`/`acquires` predicates are written -- only widening the
  root itself reaches it. This script was invisible to the original ratchet,
  not merely unwired by it.
- `run.sh` itself -- the project's own primary interactive dev-loop launcher,
  named in the issue's suggested-shape section as a candidate but not counted
  among its six named scripts.

Total closed by this branch: 8 (6 named by #834, plus these 2 the fresh
census found), for a new total of 28 wired launch sites (20 from #826/R181
plus these 8).

## What changed: 8 scripts wired to the lock

Each script below now `source`s `docker/qemu/lib/qemu-host-lock.sh` and calls
`qemu_host_lock_acquire` before its `qemu-system-aarch64` launch, with
`qemu_host_lock_track_pid` registering the launched PID immediately after
capture (per #826/R181's own fix-round: the tracked PID lets the lock's
chained `EXIT` trap kill the child even on a script with no cleanup trap of
its own, or on a SIGTERM/SIGINT delivered to just the script's own PID).

| Script | Shape | Change beyond wiring |
|---|---|---|
| `scripts/run-arm64-keyboard-test.sh` | bare `qemu-system-aarch64 \`, backgrounded, existing `trap cleanup EXIT` | wiring only -- chains onto the existing trap |
| `scripts/run-arm64-boot-test.sh` | bare `qemu-system-aarch64 \`, backgrounded, no existing trap | wiring only -- the lock installs its own `EXIT` trap |
| `scripts/run-arm64-qemu.sh` | `exec qemu-system-aarch64 \` (interactive, Ctrl-A X exits) | `exec` dropped for a plain foreground launch (see below) |
| `scripts/run-arm64-graphics.sh` | `exec qemu-system-aarch64 \` (interactive) | same `exec`-drop |
| `scripts/run-aarch64-userspace.sh` | `exec qemu-system-aarch64 \` (interactive) | same `exec`-drop |
| `scripts/test_tracing_via_gdb.sh` | `"$QEMU_BIN" \` (aarch64 leg only; the x86_64 leg launches `qemu-system-x86_64` and is untouched) | lock call placed inside the `if [ "$ARCH" = "aarch64" ]` branch only |
| `docker/qemu-aarch64/run-arm64-boot.sh` | `timeout 30 qemu-system-aarch64 \`, Docker-wrapped, backgrounded | wiring only -- same Docker-wrapped shape as `docker/qemu/run-aarch64-test.sh` |
| `run.sh` | bare `qemu-system-aarch64 \` (arm64 leg only; the x86_64 leg launches `qemu-system-x86_64`), backgrounded, interactive (`wait $QEMU_PID`, Ctrl+C to stop) | lock call and `track_pid` guarded by `[ "$ARCH" = "arm64" ]` |

### Why three scripts lost their `exec`

`scripts/run-arm64-qemu.sh`, `run-arm64-graphics.sh`, and
`run-aarch64-userspace.sh` each ended in `exec qemu-system-aarch64 \ ...`
-- replacing the shell's own process image with QEMU so the script's PID
becomes QEMU's PID (a bash-scripting convenience for an interactive session
with no further work to do after boot). This is incompatible with the
`mkdir`-based lock's release mechanism: `qemu_host_lock_acquire` installs an
`EXIT` trap that releases the lock when the *shell process* exits, and `exec`
replaces that process outright -- there is no bash process left to run the
trap when QEMU itself later exits, so the lock directory would be left
behind for the next acquirer's stale-PID reclaim path on each interactive
run, not only on a crash.

Each of the three now runs QEMU as a plain foreground command (no `exec`,
still no `&`) after `qemu_host_lock_acquire`. This is an interactive display
session precisely the same as before -- Ctrl-A X still exits QEMU exactly as
it did with `exec`, the same terminal remains attached to the same monitor,
and no output or behavior difference is visible to whoever runs the script.
The only difference is internal: on QEMU's exit, control returns to the
shell for one instruction (falling off the end of the script) instead of the
process image being replaced, so the lock's `EXIT` trap can run and release
it. `scripts/run-arm64-qemu.sh` was checked by hand for a trailing action
after its old `exec` line that this change might now skip -- there is no
such action; the `exec` was already the file's last line.

### `test_tracing_via_gdb.sh` and `run.sh`: arch-conditional locking

Both scripts pick `qemu-system-aarch64` or `qemu-system-x86_64` at runtime
(`--arch`/host `uname -m` for the former, `--x86` for the latter). The
host-wide lock in `docker/qemu/lib/qemu-host-lock.sh` serializes
`qemu-system-aarch64` boots specifically (its own `qemu_host_lock_count()`
counts aarch64 processes only); routing an x86_64 launch through it would
serialize x86_64 boots against aarch64 boots for no reason the lock's own
design intends. Both scripts `source` the helper unconditionally (harmless --
sourcing has no runtime cost) but only call `qemu_host_lock_acquire` and
`qemu_host_lock_track_pid` on the aarch64 leg; the x86_64 leg is untouched in
both files.

## What is disclosed, not fixed, in this branch

Consistent with #826/R181's own precedent of disclosing rather than silently
absorbing a gap it did not close (`docs/planning/green-program/gates/
HOST-QEMU-LOCK-2026-09-05.md`'s own "scripts/ is not covered" section is what
produced #834 in the first place):

- **Three `.py` launchers are not wired**: `scripts/debug_smp_deadlock.py`,
  `scripts/debug_elr0_crash.py`, and `docker/qemu/run-aarch64-test-runner.py`
  each build a `subprocess` argument list containing `qemu-system-aarch64`.
  `docker/qemu/lib/qemu-host-lock.sh` is a bash library; there is no `source`
  equivalent from Python, and porting its `mkdir`/stale-PID protocol to
  Python is a materially different task from wiring an existing bash helper
  into a bash caller. These three are one-off local debugging tools invoked by
  a human running one GDB session at a time, not scripts a gate or CI runs
  concurrently with anything else -- the same category
  `docs/planning/green-program/gates/GATE-TMP-BASEDIR-2026-09-05.md`'s own
  review already placed two of these three in ("one-off local debugging
  sessions, not the shared container") for an unrelated `/tmp` collision
  finding. Not fixed here; a Python-native lock implementation, if wanted,
  is its own follow-up.
- **`docker/qemu/run-aarch64-test.exp` is not wired**: a Tcl/expect script
  that `spawn`s `qemu-system-aarch64` directly. `grep -rln
  "run-aarch64-test\.exp" .` across the whole repository returns only the
  file itself -- 0 callers anywhere in this tree. Same bash/Tcl boundary as
  the `.py` scripts above, compounded by being unreferenced; not fixed here.
- **Two pre-existing kill-by-pattern hazards are unchanged**:
  `scripts/run-arm64-boot-test.sh`'s `pkill -9 -f
  "qemu-system-aarch64.*kernel-aarch64"` and `docker/qemu-aarch64/
  run-arm64-boot.sh`'s `docker kill $(docker ps -q --filter
  ancestor=breenix-qemu-aarch64)` each terminate by pattern/image match
  rather than a specific PID or container ID this script itself launched --
  the same shape #834's own issue body named for the first of these two
  ("a second, distinct issue in its own right... same shape as the
  already-filed #829") and asked to be looked at "regardless of which fix
  this issue gets," not as a precondition of this fix. Wiring the host-wide
  lock does not touch either kill line; both are left exactly as found.
- **`shell_scripts_below`'s `.sh`-extension filter is what keeps the three
  `.py` files and the one `.exp` file out of this branch's own `>= 28`
  ratchet floor**, not a path-based exemption list -- so a future `.sh`
  launcher anywhere under `docker/`, `scripts/`, or `run.sh` is still caught
  automatically by the widened census below, and the floor cannot be
  satisfied by miscounting one of these four non-`.sh` files as "covered."

## The ratchet: `tests/qemu_host_lock_structure.rs`, widened

`shell_scripts_below("docker/qemu")` (the original #826/R181 walk root)
becomes two calls -- `shell_scripts_below("docker")` and
`shell_scripts_below("scripts")` -- plus `run.sh` added as a single named
file (it is not itself a directory, so the recursive walk helper does not
apply to it). Widening the root from `docker/qemu` to `docker` is what
reaches `docker/qemu-aarch64/run-arm64-boot.sh`: a walk rooted at one
directory does not descend into a sibling directory regardless of how the
`launches`/`sources`/`acquires` predicates are written, so no predicate
change alone could have closed that reach gap -- only widening the root
could. The anti-vacuity floor moves from 20 to 28, matching the exact count
this branch wired (20 pre-existing + 8 new).

The `launches_qemu_aarch64`/`is_qemu_aarch64_launch_line` predicate itself is
unchanged in its detection logic; it incidentally already covers
`scripts/test_tracing_via_gdb.sh`'s indirect `"$QEMU_BIN" \` invocation shape
via the `QEMU_BIN=qemu-system-aarch64` assignment line that feeds it (that
assignment line itself ends with the bare token, which is exactly what the
predicate's "ends with `qemu-system-aarch64` after stripping a trailing `\`"
rule matches) -- no special-casing was needed for that file's indirection.

### Mutation records

Two independent proofs of the widened census, both against real files in the
working tree (not synthetic strings):

**In-suite** (`qemu_host_lock_predicates_are_not_vacuous`'s second mutation
leg, added by this branch): the real `qemu_host_lock_acquire` call is
stripped from `scripts/run-arm64-boot-test.sh`'s in-memory text (parallel to
the pre-existing first leg's proof against `docker/qemu/
run-aarch64-boot-test-strict.sh`), and the source line is asserted to remain
-- proving the acquire check specifically, not the source check. A further
reach-check in the same test asserts the widened census actually lists
`docker/qemu-aarch64/run-arm64-boot.sh` and `run.sh` by name.

**On-disk, live**: the real `qemu_host_lock_acquire` line was removed from
the tracked `scripts/run-arm64-boot-test.sh` on disk (not a scratch copy),
the whole-suite test re-run, then the file was restored from a
pre-mutation backup and re-verified byte-identical (`grep -c
"^qemu_host_lock_acquire$" scripts/run-arm64-boot-test.sh` read `1` again
after restore).

```
cmd:  scripts/run-arm64-boot-test.sh's `qemu_host_lock_acquire` line (exact
      text "qemu_host_lock_acquire\n", zero-indented) removed from the real
      tracked file, then:
      cargo test --test qemu_host_lock_structure
        every_aarch64_qemu_launch_script_sources_and_acquires_the_host_lock
exit: 101 (test binary FAILED; "test result: FAILED. 0 passed; 1 failed")
assertion: "aarch64 QEMU launch(es) bypass the host-wide lock:
            scripts/run-arm64-boot-test.sh: launches qemu-system-aarch64 but
            does not calls qemu_host_lock_acquire (#826/#834/R181)"
```

After restore, `cargo test --test qemu_host_lock_structure` returned to
`test result: ok. 2 passed; 0 failed`.

## Boot proofs (2026-09-05)

Kernel built at this branch's own head, in a fresh worktree needing the same
one-time setup #835's own doc recorded (`rust-fork` symlink, prebuilt
`userspace/programs/aarch64/*.elf` files, and `target/ext2-aarch64.img` --
these are not git-tracked and this branch's diff does not touch them):

```
touch kernel/src/main_aarch64.rs
cargo build --release --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
-> Finished `release` profile [optimized] target(s)

scripts/check-kernel-no-neon.sh target/aarch64-breenix-kernel/release/kernel-aarch64
-> PASS: 0 FP/SIMD load/store instructions in kernel .text (allowlisted & suppressed: 0)
```

Host aarch64 QEMU count was 0 before each run below (`pgrep -x
qemu-system-aarch64 | wc -l`), and 0 after each run.

### (a) `scripts/run-arm64-boot-test.sh quick` -- the cheapest newly-wired script

```
./scripts/run-arm64-boot-test.sh quick
```

Output (verbatim, the lock's own notice line included):

```
[2/4] Starting QEMU...
QEMU HOST LOCK: host aarch64 QEMU count before acquire: 0
[3/4] Waiting for kernel output (30s timeout)...
...
[4/4] Quick Boot Check:
========================================
PASS: 'Hello from ARM64' found
```

### (b) One strict-gate run, unaffected by the widening

Rebuilt with `--features boot_tests` (the strict gate's own requirement),
plus a fresh `target/ext2-aarch64.img` (`scripts/create_ext2_disk.sh --arch
aarch64`, default 256MB size -- not git-tracked, needed once per fresh
worktree same as the ELFs above):

```
BREENIX_GATE_TMP=<scratch> ./docker/qemu/run-aarch64-boot-test-strict.sh 1
```

```
QEMU HOST LOCK: host aarch64 QEMU count before acquire: 0
qemu-system-aarch64: terminating on signal 15 from pid 91619 (<unknown process>)
  [OK] Boot 1: SUCCESS
  [GATE_BOOT_FACTS:boot=1:host_ms=1788650287847-1788650298996:qemu_at_start=0:load_at_start=5.05:qemu_at_end=1:load_at_end=5.66:qemu_cpu_s=20.24:guest_uptime_ms=10581:ended_by=scored_pass]
=========================================
PASS: 1/1 boots succeeded
=========================================
```

This confirms the widened ratchet (root moved from `docker/qemu` to
`docker`) did not disturb `docker/qemu/run-aarch64-boot-test-strict.sh`'s
own existing wiring from #826/R181 -- the strict gate is untouched by this
branch's diff and still passes.

## Structural suites and claim-lint

```
cargo test --test <name>   for each of the 33 tests/*_structure.rs files
-> 33/33 green, 593 test cases total (summed from each suite's own "N passed"
   line; qemu_host_lock_structure.rs contributed 2 of the 593)

python3 scripts/claim-lint.py
-> "claim-lint: clean (9 file(s) checked, changed hunks vs b40fbee49aea)."
   "claim-lint: 25 pre-existing finding(s) outside this branch's changed
   hunks not reported (--whole-file shows them)." exit 0
```

The no-argument run first found 3 findings, 3 of 3 in this branch's own new
comment text in `tests/qemu_host_lock_structure.rs` -- each an unquantified
absolute word (a negation-based one and a totality-based one in two
doc-comment paragraphs, and a third totality word opening a third paragraph)
-- the same "over-broad phrasing in a repeated comment block" shape #825's
and #826/R181's own docs record finding in their own changed hunks. Each was
reworded to bounded phrasing carrying the identical meaning (see the diff on
`tests/qemu_host_lock_structure.rs`'s doc comments), and the no-argument run
then reported 0 findings in this branch's changed hunks.

```
claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files <this doc> -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg <msg> -> exit 0   (one per commit)
```

## What is NOT claimed

- **The three `.py` launchers and the one `.exp` launcher are unwired**, for
  the reasons given above (no bash `source` mechanism, one-off local
  debugging tools per existing precedent, and -- for the `.exp` file --
  0 callers anywhere in this repository). If a developer runs one of these
  four alongside a wired script, or two of the four together, the contention
  #826/R181 measured is not prevented.
- **The two pre-existing kill-by-pattern hazards
  (`scripts/run-arm64-boot-test.sh`'s `pkill -9 -f`, `docker/qemu-aarch64/
  run-arm64-boot.sh`'s `docker kill $(docker ps -q --filter ancestor=...)`)
  are unchanged.** Both can still terminate a different, unrelated process
  matching the same pattern/image; this branch's own issue disclosed the
  first of these two explicitly as a separate concern, not a precondition
  of the lock-wiring fix.
- **`run.sh --parallels` and `run.sh --vmware` are unaffected.** Both exit
  before reaching the native `qemu-system-aarch64` launch line this branch
  wires; they boot a VM via `prlctl`/VMware tooling, not a native QEMU
  process, and are outside this lock's scope by construction (there is no
  `qemu-system-aarch64` process on the host to serialize against in either
  path).
- **This branch is not a soak.** The strict-gate run above is 1 boot, and
  the quick-mode run is 1 invocation; neither is a many-iteration statistical
  claim about contention frequency the way #826/R181's own 200-boot health
  battery was for the original lock.
- **#827's gate-side instrumentation gap remains untouched**, as it was in
  #826/R181's own branch -- this work adds lock coverage, not the per-boot
  host-fact fields #827 asks for.
- **This branch adds no new x86-only script and touches no x86-only launch
  line.** `scripts/test_tracing_via_gdb.sh`'s and `run.sh`'s own x86_64 legs
  are read but not edited; both still launch `qemu-system-x86_64`
  unconditionally, exactly as before this branch.

claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files docs/planning/green-program/gates/HOST-QEMU-LOCK-SCRIPTS-834-2026-09-05.md -> exit 0
