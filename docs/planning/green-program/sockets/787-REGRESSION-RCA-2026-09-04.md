# The x86 boot-tests gate wedges inside `x86_retire_cohort` after PR #787

Date: 2026-09-04. Branch: `fix/787-retire-cohort-freeze`, based on `d6b7a186`.

## Signature

`docker/qemu/run-x86-boot-tests.sh 1` reaches
`[TEST:process:x86_retire_cohort:START]` and then stops producing serial output
entirely, at a child index that differs run to run. The QEMU process stays at
~101% CPU. The kernel serial file is byte-identical from the moment the wedge
sets in until the harness kills the run 830-900 s later.
claim-lint:ok: the flat-size window is the last two columns of
`serials/787-regression/ab-serials/main-{1,2,3}.sizehist`, 3 of 3.

## The A/B that bisected it

Recorded in the same session at
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/p775/x86-boot-tests-ab.md`,
with the four timed-out runs' serials copied into
`docs/planning/green-program/sockets/serials/787-regression/ab-serials/`.

| leg | commit | runs | cohort |
|---|---|---|---|
| pre | `ee6de882` (merge of PR #785) | 3 | cleared in 131 s, 131 s, 141 s |
| main | `b257e69e` (merge of PR #787) | 3 | never reached `PASS`; wedged at children 49, 13, 14 of 64 |

Only PR #787 separates the two heads.

## The merge itself is a clean three-way merge

`git merge-tree --write-tree ee6de882 18d35cac` produces tree
`0d3c4c34335830902c6414f2d5caa0c8d516a99f`, and `git rev-parse b257e69e^{tree}`
is the same hash. `git show --cc b257e69e | wc -l` prints 57, and those 57
lines are the commit header: no hunk in the merge differs from both parents. So
the landed tree is what git computed, with no hand resolution, and a bad
conflict resolution is not the cause.
claim-lint:ok: both commands were re-run in this slot; the tree hashes are
quoted verbatim above.

## The mechanism, in causal order

1. `kernel/src/interrupts/context_switch.rs:1266` `setup_kernel_thread_return`
   runs inside the timer interrupt, with interrupts disabled, and calls
   `crate::memory::process_memory::switch_to_kernel_page_table()` at
   `context_switch.rs:1308` (line numbers at `b257e69e`).
2. `kernel/src/memory/process_memory.rs:2831`
   `switch_to_kernel_page_table` reads the master kernel PML4 through
   `kernel_page_table::master_kernel_pml4()`.
3. At `b257e69e`, `kernel/src/memory/kernel_page_table.rs:780` reads it as
   `MASTER_KERNEL_PML4.lock().clone()` -- a `spin::Mutex` acquisition
   (`kernel_page_table.rs:28`). A `spin::Mutex` has no owner field and no
   reentrancy check; `lock()` spins until the byte clears.
4. The same mutex is taken from ordinary thread context, with interrupts
   ENABLED, by `map_kernel_page` (`kernel_page_table.rs:126`) and
   `unmap_kernel_page` (`kernel_page_table.rs:275`), each written as
   `if let Some(master_frame) = MASTER_KERNEL_PML4.lock().clone() { ... }`.
   In editions before Rust 2024 the scrutinee temporary of an `if let` lives
   until the end of the whole `if let` expression, so the guard is held across
   both arms -- including the `log::trace!` in the then-arm and the `Cr3::read()`
   in the else-arm. Measured in the shipped `b257e69e` binary: the acquire is
   the `call core::sync::atomic::AtomicBool::compare_exchange_weak` at
   `map_kernel_page+0x2cd`, the guard's drop is the indirect call at
   `map_kernel_page+0x4ee`, and the `log::trace!` machinery
   (`AtomicUsize::load` of the max level at `+0x42e`, the `log` calls at
   `+0x452` and `+0x4d8`) sits between them -- 545 bytes of code under the lock.
   claim-lint:ok: the disassembly is
   `docs/planning/green-program/sockets/serials/787-regression/mapdis-b257e69e.txt`.
5. `x86_retire_cohort` forks and tears down 64 children, and each child's kernel
   stack is 128 pages, so this window is entered thousands of times per cohort.
   `[DEBUG] kernel::memory::kernel_stack: Mapping 128 pages for kernel stack 5`
   is the last recurring line before the wedge in 3 of the 3 A/B main serials.
   claim-lint:ok: `serials/787-regression/ab-serials/main-{1,2,3}/serial_kernel.txt`.
6. A timer preemption inside that window leaves the mutex held by a thread that
   is no longer running. The dispatch performed by that same interrupt, or by a
   later one, reaches step 1, spins on the held byte with IF=0, and no interrupt
   can run again to resume the holder. The CPU spins; no further byte is
   printed; QEMU shows 101%.
   claim-lint:ok: IF=0 is the `eflags 0x2` reading in both specimen registers
   dumps, `serials/787-regression/spec{1,2}/gdb_sample_1.txt`, 2 of 2.

## Why the branch alone, and main before #787, mostly did not hit it

The consumer in step 1 is only reached for dispatches that go through
`setup_kernel_thread_return`. In `switch_to_thread` the idle thread reaches it
only on the `<1>` arm (`context_switch.rs:881`, idle WITH a saved context); the
`<I>` arm (`context_switch.rs:887`, `setup_idle_return`) does not touch the
master PML4 at all.

Counting the single-character dispatch markers in the A/B serials over each
whole boot:

| leg | `<I>` | `<1>` | `<K>` |
|---|---|---|---|
| pre `ee6de882` (`ab-serials/pre-1/serial_user.txt`) | 461 | 23 | 54 |
| main `b257e69e` (`ab-serials/main-1/serial_user.txt`) | 0 | 59 | 62 |
| specimen 1 (`spec1/serial_user.txt`) | 0 | 70 | 73 |
| specimen 2 (`spec2/serial_user.txt`) | 0 | 19 | 22 |

`<I>` is taken when the idle thread's saved `context.rip` is still 0 or the
`idle_loop` entry address -- the two values the predicate tests -- i.e. when no
save has written it.
claim-lint:ok: the predicate is the `has_saved_context` binding at `b257e69e`
`kernel/src/interrupts/context_switch.rs:865-876`, consumed by the
`if has_saved_context` branch at `:878`.
On the pre leg that was the common case, because the boot/init thread (tid 1)
had `blocked_in_syscall` left set by `Scheduler::block_current_for_io_publish`
(`ee6de882` `kernel/src/task/scheduler.rs:3393`) during the ext2 root read, and
that flag routes its later saves into `save_kernel_context_with_guard`
(`context_switch.rs:474`), which writes into `process.main_thread` and stores
nothing for a thread with no process. PR #787 fixed that producer --
`thread.blocked_in_syscall = thread.owner_pid.is_some()` at `b257e69e`
`scheduler.rs:3401` and `:3444` -- which is correct in itself, and its
consequence is that tid 1's context is now really saved, so its dispatches take
`<1>` instead of `<I>`. That moves the master-PML4 read from "rarely on the
dispatch path" to "on essentially every idle dispatch", which is what turns a
latent lock-context defect into a wedge the cohort hits within 15-65 s.

What this section does NOT establish: the exact per-run probability on each leg,
or why the branch head `18d35cac` measured 18/20 rather than 0/20 on this gate.
The marker counts above are from one boot per leg on the A/B, plus the two
specimens. The `<I>` -> `<1>` flip is measured; the attribution of that flip to
the `blocked_in_syscall` producer change is read from the source and from the
specimen's restored context (below), not from a mutation run.

## The two specimens

Both captured by attaching GDB to a wedged QEMU (`-gdb tcp::PORT`, symbols
anchored at the `virtual_address_offset: 0x10000000000` the bootloader printed).
Raw logs are in `serials/787-regression/spec1/` and `spec2/`.

Specimen 1 -- `d6b7a186` (main HEAD), fresh clone `/root/breenix-787`, wedged
after `child_54`:

```
rip      0x100002cd482  <core::sync::atomic::spin_loop_hint+2>
eflags   0x2                       <- IF clear: interrupts disabled
rdi      0x100004b3db8             <- &MASTER_KERNEL_PML4
#0 core::sync::atomic::spin_loop_hint
#1 kernel::memory::kernel_page_table::master_kernel_pml4 + 122
#7 kernel::memory::process_memory::switch_to_kernel_page_table + 21
```

The stack above that frame carries
`setup_kernel_thread_return+474`, `switch_to_thread+2068` and
`check_need_resched_and_switch+5915`, which is the dispatch path.

The lock byte itself, read at the static's address:

```
0x100004b3da0 <KERNEL_PDPT_FRAME>:   0x00 ...   <- lock byte 0 (free)
0x100004b3da8 <KERNEL_PDPT_FRAME+8>: 0x01 ...   <- Option discriminant = Some
0x100004b3db0 <KERNEL_PDPT_FRAME+16>: 0x00 0xa0 0x61 ...  <- frame 0x61a000
0x100004b3db8 <MASTER_KERNEL_PML4>:  0x01 ...   <- lock byte 1 (HELD)
```

The `KERNEL_PDPT_FRAME` bytes are what identify the layout: lock byte first,
then the `Option<PhysFrame>`.

The thread being dispatched is tid 1 (per-CPU current-thread pointer
`0x444444600d38`, id word `1`), and the context
`setup_kernel_thread_return` was restoring is
`rip = enable_and_hlt+2, rflags = 0x284, rsp = 0xffffc90000284ef0` -- the idle
thread parked in `idle_loop`. That is the `<1>` arm from the table above.

A scan of the whole 512 MiB direct physical map for words pointing inside
`master_kernel_pml4` returned 2 hits: one relocation word in the kernel image
and the return address on the wedged stack. No saved thread context is parked
inside the accessor, which is consistent with the holder being one of the
thread-context readers in step 4 rather than a thread stopped inside step 3.

Specimen 2 -- `b257e69e` exactly, the pre-existing A/B clone
`/root/breenix-ab-main`, wedged after `child_9`, same signature:

```
rip      0x100002cd492  <core::sync::atomic::spin_loop_hint+2>
eflags   0x2
rdi      0x100004b3db8
#1 kernel::memory::kernel_page_table::master_kernel_pml4 + 122
#7 kernel::memory::process_memory::switch_to_kernel_page_table + 21
```

## The fix

Two changes, both removing work from the interrupt-context dispatch path.

1. `kernel/src/memory/kernel_page_table.rs`: the master PML4 frame moves from
   `Mutex<Option<PhysFrame>>` to a write-once `AtomicU64` physical address,
   `MASTER_KERNEL_PML4_PHYS`. `build_master_kernel_pml4()` stores it once with
   Release; `master_kernel_pml4()` is an Acquire load and a `PhysFrame`
   reconstruction, with no lock. `map_kernel_page` and `unmap_kernel_page` now
   call that accessor instead of locking the cell themselves, so the `if let`
   temporary they hold is a plain `Option<PhysFrame>`, and no guard outlives the
   accessor call. That removes this deadlock at its source: the dispatch path
   has no lock left to spin on.
   claim-lint:ok: the 3 code lines that name the cell are pinned by
   `tests/dispatch_path_lock_free_structure.rs`.
2. `kernel/src/interrupts/context_switch.rs`: `setup_kernel_thread_return` no
   longer clones the thread's `name`. That clone allocated a `String` -- taking
   the heap allocator's lock from interrupt context, the same hazard class -- on
   each kernel-thread dispatch, for a value the function bound as `_name` and
   dropped. `Context` is plain registers, so the remaining clone performs no
   allocation.
   claim-lint:ok: 1 of 1 `name.clone()` in this function at `b257e69e`
   `kernel/src/interrupts/context_switch.rs:1272-1274`; the absence is pinned by
   `tests/dispatch_path_lock_free_structure.rs`.

Neither file is on the Tier-1 prohibited list. `context_switch.rs` is Tier-2 and
the edit removes work from the hot path rather than adding any.

`tests/dispatch_path_lock_free_structure.rs` is the guard: it asserts the
accessor takes no lock and reads the atomic, that the cell is named on exactly
three code lines (declaration, one store, one load), that both
`map_kernel_page`/`unmap_kernel_page` reach it through the accessor, that the
x86 `switch_to_kernel_page_table` arm holds no lock, and that
`setup_kernel_thread_return` contains no allocating call. Anti-vacuity, run in
this slot: with `kernel/src/memory/kernel_page_table.rs` and
`kernel/src/interrupts/context_switch.rs` restored from `b257e69e`, 3 of the 4
tests fail; with the fix in place, 4 of 4 pass. The fourth,
`switch_to_kernel_page_table_takes_no_lock`, passes on both trees -- that arm
already routed through the accessor at `b257e69e`, so it is a forward guard, not
a mutation-proven one.

## Proof: 6 of 6 single gate runs, 8 of 8 four-boot runs, 3 of 3 production runs, 80 of 80 aarch64 strict boots

Plus the ratchet mutation at the end of this section.

The runs are on beast, in the `breenix-x86` Incus container, sharing the machine
with the `/root/breenix-737-oracle` tenant throughout. Gate stdout is committed
under `docs/planning/green-program/sockets/serials/787-regression/prove/`.

x86, `docker/qemu/run-x86-boot-tests.sh 1`, six sequential runs -- the gate that
was red 3 of 3 on `b257e69e` and 5 of 5 across the two specimens:

| run | cohort START->PASS | total | verdict |
|---|---|---|---|
| 1 | 128 s | 478 s | `x86 frame-custody gate run 1: PASS` |
| 2 | 133 s | 497 s | PASS |
| 3 | 133 s | 486 s | PASS |
| 4 | 131 s | 483 s | PASS |
| 5 | 127 s | 483 s | PASS |
| 6 | 127 s | 476 s | PASS |

6 of 6 cleared `x86_retire_cohort`, in a 127-133 s band that sits inside the
pre-#787 leg's own 131-141 s. The verdict lines are in
`docs/planning/green-program/sockets/serials/787-regression/prove/`, one
`single-N.stdout` per run, and
`docs/planning/green-program/sockets/serials/787-regression/prove/single-1/`
carries that run's full serial pair.

x86, `docker/qemu/run-x86-boot-tests.sh 4`, two batches: 4 of 4 PASS in each,
8 of 8 overall, in
`docs/planning/green-program/sockets/serials/787-regression/prove/gate4-1.stdout`
and `gate4-2.stdout`. Batch 1 took 1837 s wall.

x86 production profile, `docker/qemu/run-x86-prod-profile-boot-test.sh`, 3 of 3
runs exit 0, at 107 s / 109 s / 108 s, in
`docs/planning/green-program/sockets/serials/787-regression/prove/prod-1.stdout`
and its two siblings. That script builds with no `--features` at all, so it is
also the zero-feature build check; the
`boot_tests,testing,external_test_bins` build printed no `warning`/`error`
line when re-run in this slot (`grep -E "^(warning|error)"` -> no output).

aarch64: `cargo build --release --features boot_tests --target
aarch64-breenix-kernel.json -Z build-std=core,alloc -Z
build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` is
clean, and `docker/qemu/run-aarch64-boot-test-strict.sh` was run 4 times at 20
boots each: 20/20, 20/20, 20/20, 20/20 = 80 of 80. The fourth invocation ran
against a kernel rebuilt from the committed tree.

Anti-vacuity for the fix itself -- the fix must not work by avoiding the code
path. On the fixed build's run 1, the `x86_retire_cohort` window still contains
97 `<1>` dispatches and 0 `<I>`, the same shape as the wedging `b257e69e` build
(48 `<K>` / 47 `<1>` / 0 `<I>` before it froze). So the dispatch still reaches
`setup_kernel_thread_return` -> `switch_to_kernel_page_table` ->
`master_kernel_pml4()` on each idle dispatch; what changed is that the read no
longer takes a lock.
claim-lint:ok: the counts are re-derivable from
`docs/planning/green-program/sockets/serials/787-regression/prove/single-1/serial_user.txt`
and
`docs/planning/green-program/sockets/serials/787-regression/ab-serials/main-1/serial_user.txt`.

Ratchet mutation, run in this slot: with both edited files restored from
`b257e69e`, 3 of the 4 assertions in
`tests/dispatch_path_lock_free_structure.rs` fail; with the fix, 4 of 4 pass.

Which tree each leg ran on: the six single runs used the reproduction working
tree, whose only difference from the committed branch is comment text added
while discharging claim-lint. The two 4-boot batches and the three production
runs ran after `git checkout -f FETCH_HEAD` in that clone, i.e. on commit
`3d915db6` exactly.

## What this record does not claim

- It does not claim the lock-context defect was introduced by PR #787. The
  `spin::Mutex` read from `switch_to_kernel_page_table` predates it; #787 changed
  how often the dispatch path takes that read.
- It does not claim `map_kernel_page` is the only possible holder. It is the
  reader with by far the most hold-time during the cohort, and the RAM scan is
  consistent with a thread-context holder, but the specimens do not name the
  holding thread.
- `KERNEL_PDPT_FRAME` remains a `spin::Mutex`. It is not read from the dispatch
  path in this tree, so it is not part of this deadlock; it is left alone here
  and noted in the filed issue.
