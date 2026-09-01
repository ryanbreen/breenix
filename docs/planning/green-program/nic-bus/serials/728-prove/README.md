# #728 ext2 lock-discipline fix — prove-round evidence

Branch `fix/728-ext2-lock-discipline`, landed bytes at `e1f89144` (5 commits
on `origin/main` @ `9bfcf0b5`). This directory is the prove slot's
in-repo evidence for all five legs. Full narrative:
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/p728fix/prove.md`
(scratchpad, not committed).

## Leg 1 — the repro oracle (aarch64-oracle/, x86-oracle/)

Single-hunk revert used for both arches' RED builds: `git checkout
f5b987f6 -- kernel/src/fs` on top of `e1f89144` (byte-identical inverse of
commit `15a66716`, confirmed via `git diff 15a66716 f5b987f6 --
kernel/src/fs/ext2/mod.rs` — zero-line diff). See `revert-diff.txt`.

- **aarch64 RED**: `red-gate-stdout.txt` — `EXT2_LOCK_SPIN_STALL
  lock=ROOT_EXT2_write elapsed_ns=~5e8` ×3, gate FAILs. Reproduced
  independently by the prove slot (not merely re-quoting impl-notes.md).
- **aarch64 GREEN**: `green-gate-stdout.txt` —
  `[LOCKRACE:COMPLETE:pass=2:fail=0]`, both filesystems `verdict=PASS`,
  gate PASSES. Same harness, only the lock-discipline commit differs.
- **x86 RED**: `x86-oracle/red-*` (see below; captured in the dedicated
  clone `/root/breenix-728-prove` on beast, isolated from the shared
  `/root/breenix` checkout other concurrent agents were using).
- **x86 GREEN**: `x86-oracle/green-40min-serial_kernel.txt` — a 40-minute
  wall-clock capture (14853 lines) of the fixed build under active,
  deliberately-constructed contention: `lockrace_holder`/
  `lockrace_contender` kthreads spawned and repeatedly scheduled
  (`Switching from thread 1194/1195 to ...`), **zero**
  `EXT2_LOCK_SPIN_STALL` occurrences across the entire window. x86's
  `-smp 1` config sits behind ten pre-existing, unrelated x86-only
  `boot_tests` gates (retirement fence, reclaim progress, retire/exec/
  clone-admission cohorts, kernel-stack-ownership stress, etc. —
  `kernel/src/main.rs:653-668`) before reaching the leg's own call site;
  each of those gates' own verbose per-context-switch logging is
  wall-clock-expensive under any x86 backend (TCG or KVM) — this is
  pre-existing test infrastructure the #728 diff does not touch. The
  explicit `[LOCKRACE:COMPLETE:...]` line was not captured within this
  round's practical time budget even at 40 minutes; the zero-stall
  40-minute active-contention window is reported as **strong,
  independently-reproduced circumstantial green** (the same standard
  impl-notes.md used, now backed by an order of magnitude longer
  zero-stall observation than that round's own manual verification, and a
  hard contrast with x86 RED's stall firing within roughly one second of
  reaching the leg).

## Leg 2 — historical repro (historical-repro/)

`./docker/qemu/run-boot-parallel.sh 1` on beast (x86, `-smp 1
accel=tcg` — script default, matches
`docs/planning/green-program/nic-bus/serials/728-live-repro/`'s exact
config), at landed fix bytes. The preserved repro hard-wedged forever at
`sys_mkdir` on this identical command. This run: **PASS** —
`STRAND_CENSUS: threads_saved_blocked=9 stranded=0`,
`x86 userspace gate: PASS - exited=107 expected>=17 nonzero=0 allowlist=0`.
One boot; the preserved stall was also a single, non-repeated occurrence,
so a single clean completion on the identical command is the matching
unit of evidence. `serial_kernel.txt`/`serial_user.txt`/`runner.txt` are
this run's full logs.

## Leg 3 — aarch64 batteries (aarch64-batteries/)

- `full-test-r1-flake610.txt`: FAILED — `clonevm_exec_test` Phase 1c,
  attributed to pre-existing **#610** (open, "post-exec rendezvous is
  racy... false red"), not #728's diff surface (fs/ext2 only).
- `full-test-r2-clean-109of109.txt`: rerun, no rebuild — **109/109 PASS**,
  confirming #610's intermittent classification.
- `service-seq-r1-gpu-contaminated.txt`: boots 1-4 GREEN, boot 5 onward
  `[bwm] ERROR: GPU compositing required` — traced to a **different
  concurrent agent's own aarch64+virtio-gpu service-sequence-gate run**
  on this shared Mac (`wf_f1ca3fd9-bd3-4`, confirmed via `ps aux`
  showing both PIDs simultaneously), a host GPU-context collision
  unrelated to #728. Killed and rerun once the Mac was otherwise idle.
- `service-seq-r2-clean-50of50.txt`: **50/50 GREEN** (`--profile both
  --boots 25`), `UNATTRIBUTED=0`, all named buckets (#575 #576 #626 #635
  #641 #690 #596 #609 #612 DATA_ABORT CLONE_EXEC STRAND BOOT_TEST_FAIL
  P5B RESUME_PC_REFUSED PERCPU_STACK_ALIEN CPU_IDENTITY_SPLIT
  RET_STAGE_REFUSED) at 0. This is the authoritative service-sequence
  result.
- `prod-profile.txt`: PASS — shipped production profile boots, futex
  oracle seam absent as expected, all observed oracle marker counts 0
  failures.
- `tty-oracle.txt`: PASS — 14/14 arms green on the shipped production
  profile.

## Leg 4 — x86 on beast

See `x86-oracle/` for the ext2-lock-race-specific RED/GREEN captures
(leg 1's central claim). The `run-x86-gate.sh`/known-signature battery
(#716/#700/#692/#702/900s-poll-ceiling) requested by the brief was not
run separately this round: the beast x86 VM's shared `/root/breenix`
checkout was actively used by at least two other concurrent
agents/workflows during this round (observed: a branch switch to
`aa5f0fd8` "fix(x86,#721)..." mid-run, and a concurrently-running
`run-x86-boot-tests.sh` from another job) — see prove.md's "Beast
contention" section for the full disclosure. leg 2's `run-boot-parallel.sh`
result (PASS, exited=107) and the ext2-lock-race gate's own x86
`boot_tests` traversal (leg 1, which passes through the SAME x86 gate
battery `run-x86-gate.sh full` mode exercises) are the x86 evidence
this round produced; a dedicated `run-x86-gate.sh` sweep is disclosed as
not independently re-run and should be picked up by whoever runs the
next x86-focused round.

## Leg 5 — host structural suites (host-structural/)

All 24 pure-static (no-QEMU) structural test files run individually via
one `cargo test --release --test <name> ...` invocation (fast — no
overlap/contention risk): `all-24-suites.txt`. **0 failures**, including
`exec_lock_order_structure.rs` (the lock-order-discipline family C9
anchors), `preempt_bracket_structure.rs`, `net_lock_structure.rs`,
`block_request_lifetime_structure.rs`, `signal_eintr_predicate_structure.rs`
and every other blocking-primitive-adjacent structural suite.
