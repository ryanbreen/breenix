# #713 fix-round-2 — prove-slot evidence (re-proving the landed diff)

Preserved by the #713 fix-round-2 prove pass, at branch head
`8976ee83f1e0a91689225024fa11a79b39e4d9ce`. This directory sits alongside
the fix-round-1 evidence in `docs/planning/713-x86-spawn/serials/` and
covers only what fix-round-2 changed: the six-plus-one `teardown_structure`
census entries (#727), the two false `#[allow(dead_code)]` removals, the
tightened C2 teardown arm (N4) with checked argv pointer arithmetic (N7),
and the honest spawn-smoke reap pin (N1/N2). Round-1's own 12-boot/mutation
battery in the parent `serials/` directory stands unchanged and was not
re-run.

`*.log` files were renamed `.txt` (this repo's `.gitignore` has a blanket
`*.log` rule).

## `host-suites/` — all 26 structural suites, individually, on the Mac host

Every `tests/*_structure.rs` / `tests/*_clean.rs` / adjacent structural
suite, run individually via `cargo test --test <name> --release`, all
green:

`block_request_lifetime_structure` (12), `context_restore_structure` (96),
`coreproof_coverage_structure` (4), `coreproof_mutation_register_structure`
(5), `coreproof_production_clean` (4), `coreproof_sites_structure` (4),
`degenerate_transfer_fd_validation_structure` (4),
`dma_and_log_sink_structure` (4), `exec_lock_order_structure` (34),
`exit_tally_structure` (6), `fs_fault_production_clean` (4),
`kernel_no_neon_guard` (1), `loopback_pump_structure` (63),
`masked_binary_load_structure` (4), `net_lock_structure` (19),
`percpu_stack_custody` (16), `preempt_bracket_structure` (8),
`repo_symlink_hygiene` (4), `serial_line_atomicity_structure` (9),
`signal_eintr_predicate_structure` (2), `stack_bounds_tests` (17),
`strand_handoff_structure` (38), `syscall_return_register_structure` (6),
`teardown_structure` (**81**, includes
`deliberately_broken_variants_fail_the_ratchet`), `tty_oracle_structure`
(8), `x86_gate_verdict_test` (5).

Plus two extra `teardown_structure` runs proving the six new #727 census
entries (and the follow-on `REMOVE_FROM_READY_QUEUE_CALL_SITES` entry) are
exact, not vacuous:

- `teardown_structure-mutation-row-destructor.txt` — the
  `ROW_DESTRUCTOR_CALLS` entry for `sys_spawn` was bumped `1 -> 2` (source
  file, not the tested kernel — a pure test-fixture mutation). Result:
  **80 passed, 1 failed** —
  `debt4_row_removal_routes_through_the_join_on_both_arches` reddens with
  `row destructor caller census changed ~ kernel/src/syscall/handlers.rs ::
  #[cfg(target_arch=x86_64)] fn sys_spawn (expected 2, found 1)`. Confirms
  the census counts are exact-match, not floor/ceiling.
- `teardown_structure-after-revert.txt` — same mutation reverted
  byte-for-byte (verified via diff against the pre-mutation copy before
  reverting); back to 81/81 green, `git diff` empty against
  `tests/teardown_structure.rs` at HEAD.

`coreproof_production_clean`'s first attempt in this round failed
environmentally (missing aarch64 userspace test ELFs / x86 std userspace
build in a fresh worktree checkout — the worktree needed
`userspace/programs/build.sh --arch aarch64` and `--arch x86_64` run once,
plus a `rust-fork` symlink into the shared fork checkout the way the main
checkout already has one, since the fork's own `Cargo.toml` resolves
`libc = { path = "../../libs/libc" }` relative to the *physical* location
its `library/` directory canonicalizes to). Not a kernel/test regression —
confirmed green (4/4) once the worktree had the same build prerequisites
the main checkout already carries.

## `beast-leg2/` — extended prod-profile gate ×5 at landed bytes

Five sequential `docker/qemu/run-x86-prod-profile-boot-test.sh` runs on
beast's `breenix-x86` Incus VM, all `exit=0`. Every run's pins:

```
PASS: x86 production profile reached steady state with the teardown census at rest
  init bsshd-launch warning (must be absent, #713): 0
  bsshd started (#713):          1
  bsshd listening (#713):        1
  spawn-smoke child reaped exit 0 (#713): 1
  spawn-smoke reap failed (must be absent, #713 fix-round-2): 0
```

The strengthened reap pin (N1) holds at the intended literal split: success
count exactly 1, failure count exactly 0, on all five boots.

## `beast-leg3/` — mutation anti-vacuity + N4 dangling-children mechanism

- `leg3-mutation1.txt` — Tier-1 dispatch line
  (`kernel/src/syscall/handler.rs`'s `Some(SyscallNumber::Spawn) => ...`)
  reverted to the pre-#713 `SyscallResult::Err(super::ErrorCode::NoSys as
  u64)` via `sed`, applied and confirmed via `git diff` (exact one-line
  match to the historical Tier-1 revert), rebuilt, gate run. **Gate FAILs**
  (`set -e` abort, exit 1) with all three expected downstream literals
  present: `[init] Warning: failed to start spawn smoke: ENOSYS`, `[init]
  Warning: failed to start bsshd`, `[init] Failed to spawn boot script:
  ENOSYS`; the reap literal and its failure counterpart both read 0 (no
  fabricated success). Mutation reverted (`mv` back from the `sed -i.bak`
  backup), `git diff` empty confirmed.
- `leg3-mutation1-revert-confirm.txt` — rebuild after revert, gate run:
  clean `PASS`, all five #713 pins correct (same block as leg2 above).
- `leg3-mutation2.txt` — `kernel/src/process/manager.rs`'s x86
  `build_process_with_argv_at`'s `process.set_main_thread(thread);` (the
  #713 call site, line 759, distinguished from the three *other*
  `set_main_thread` call sites in the file by line number) replaced with
  `let _ = thread;` via a targeted `sed` on that exact line, confirmed via
  `git diff` (single-line change, correct call site). Rebuilt, gate run.
  **Gate FAILs** (`set -e` abort, exit 1) — but for the *expected* reason:
  this call site is shared by every x86 `spawn()`, so `bsshd`'s own spawn
  attempt fails too (`[init] Warning: failed to start bsshd`), which is
  what actually trips the gate's `INIT_BSSHD_WARNING_LITERAL == 0`
  assertion. The spawn-smoke-specific literal is exactly the one the C2
  arm's own errno claim predicts: `[init] Warning: failed to start spawn
  smoke: ENOMEM` (not the `spawn-smoke reap failed` literal — the syscall
  itself fails, so `waitpid` is never reached, and the reap-failed literal
  count is 0). The gate's own `steady state reached: 1` marker and prompt
  `exit=1` (not a timeout/kill) confirm **no hang**: the run completed on
  its own within the gate's normal wall-clock budget.
- `leg3-mutation2-revert-confirm.txt` — rebuild after reverting mutation 2
  (`mv` back from the `.bak2` copy, `git diff` empty confirmed against both
  touched files), gate run: clean `PASS`, all five pins correct again.
- `leg3-mutation2-n4strip.txt` — a second, *nested* mutation stacked on
  top of mutation 2, isolating N4's specific contribution: the three lines
  N4 added to `sys_spawn`'s C2 arm (`if let Some(parent) =
  manager.get_process_mut(parent_pid) { parent.children.retain(|&pid| pid
  != child_pid); }`) were commented out (leaving `remove_from_ready_queue`
  and `remove_process` — the pre-N4 shape) while mutation 2 stayed active.
  Rebuilt, gate run: still `FAIL` for the same `INIT_BSSHD_WARNING_LITERAL`
  reason as mutation 2 alone, and `steady state reached: 1` is *unchanged*
  from the mutation-2-alone run — **the shipped gate's own literals do not
  currently distinguish "children retracted" from "children left
  dangling"**, because nothing in `docker/qemu/run-x86-prod-profile-boot-test.sh`
  probes init's post-boot reap-loop state directly (its "steady state"
  check watches the serial console prompt, which is produced independently
  of whether init's own `waitpid(-1)` call is blocked or cycling). Both
  mutations were reverted together (`mv` back from `.bak2` copies on both
  files, `git diff` empty confirmed on both), and a final rebuild + gate
  run (see below) confirmed clean recovery.

**The N4 dangling-children mechanism itself was confirmed structurally**,
by reading `sys_waitpid` (`kernel/src/syscall/handlers.rs:3034`) directly
against the landed code: the function's very first branch is `if
current_process.children.is_empty() { return ECHILD }`; the `pid == -1`
arm below it, on a non-empty `children`, iterates every entry looking for
a `Terminated` row via `manager.get_process(child_pid)`, and if none is
found and `WNOHANG` is not set, calls `sched.block_current_for_child_exit()`
followed by an unbounded `loop { yield_current(); arch_halt_with_interrupts();
... }` that only returns when *some* child in the list terminates. A
`children` entry pointing at a row `sys_spawn`'s C2 arm already destroyed
via `remove_process` can never satisfy that loop — `manager.get_process`
returns `None` for it forever — so if it were the *only* entry (as it is
for init immediately after boot, before `bsshd` and the boot script are
attempted, and as it stays for init under mutation 2's own conditions once
`bsshd`'s spawn also fails through the same shared call site), that
`waitpid(-1)` call would block forever rather than return `ECHILD`
promptly. N4's `parent.children.retain(...)` is precisely what keeps
`children` accurately reflecting reality, so the `is_empty()` fast path
fires instead. This is a direct, from-source proof of the mechanism the
review's N4 finding named; it is disclosed as structural rather than a
live differential boot demonstration, because the shipped gate has no
existing probe that would show a difference between the two boots'
serial output.

- `leg3-mutation2-revert-confirm.txt` (listed above) also serves as the
  final full-recovery confirmation after both the mutation-2 and the
  N4-strip mutation were reverted together and rebuilt.

## `aarch64/full-test.txt` — shared-code-path check

`kernel/src/process/manager.rs`'s `write_byte_to_stack`/`write_u64_to_stack`
(the two `#[allow(dead_code)]` removals in this round) are called from
`setup_argv_on_stack`, which is itself called from **both**
`#[cfg(target_arch = "x86_64")]` **and** `#[cfg(target_arch = "aarch64")]`
callers (`create_process_with_argv`/`exec_process_with_argv` on each arch)
— confirmed by grepping the nearest preceding `#[cfg(target_arch = ...)]`
attribute above every one of `setup_argv_on_stack`'s four call sites in
`manager.rs`. This round's diff therefore does touch a non-x86-only shared
code path (removing a false dead-code suppression from functions aarch64
also compiles and calls), so `run-aarch64-full-test.sh --rebuild
--boot-tests-only` was run per the prove brief's condition.

By contrast: `kernel/src/syscall/handlers.rs`'s `sys_spawn` sits directly
under its own `#[cfg(target_arch = "x86_64")]` (confirmed at
`handlers.rs:2542`-`2543`), and `userspace/programs/src/init.rs`'s
`run_spawn_smoke` — both its call site in `main()` and its own function
definition — are independently confirmed `#[cfg(target_arch = "x86_64")]`
(lines 123-124 and 503-504). Neither reaches the aarch64 build at all.

**Result: `ARM64 BOOT TESTS: PASSED`, 109/109 tests, 0 failures/panics/
`UNATTRIBUTED` markers in the log.** Build carried only the pre-existing
`core` future-incompat notice (established pre-existing on `origin/main`
in fix-round-1). The worktree needed a one-time `ext2-aarch64.img`
(`./scripts/create_ext2_disk.sh --arch aarch64`) and aarch64 userspace
build (`./userspace/programs/build.sh --arch aarch64`) before the full
test could run at all — both gitignored, neither a tree change.

See `docs/planning/713-x86-spawn/serials/README.md` (the parent
directory) for fix-round-1's own evidence, and the #713 PR/issue thread on
GitHub for the review, prove, and fix-round writeups this evidence backs.
