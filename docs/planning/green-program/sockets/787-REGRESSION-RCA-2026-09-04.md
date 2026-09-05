# The x86 boot-tests gate wedges inside `x86_retire_cohort` after PR #787

Date: 2026-09-04. Branch: `fix/787-retire-cohort-freeze`, based on `d6b7a186`.

## Signature

`docker/qemu/run-x86-boot-tests.sh 1` reaches
`[TEST:process:x86_retire_cohort:START]` and then stops producing serial output
entirely, at a child index that differs run to run. The kernel serial file is
byte-identical from the moment the wedge sets in until the harness kills the run
830-900 s later, and the QEMU process is not exiting or blocking during that
window -- it is spinning, which is what the specimens below show directly.
claim-lint:ok: the flat-size window is the last two columns of
`serials/787-regression/ab-serials/main-{1,2,3}.sizehist`, 3 of 3; the spinning
is the `spin_loop_hint` RIP in `spec{1,2}/gdb_sample_1.txt`, 2 of 2. Round 1
also reported "the QEMU process stays at ~101% CPU"; that was a live `top`
reading during the A/B session and no committed artifact carries it, so it is
dropped rather than restated.

## The A/B that bisected it

Recorded in the same session at
`/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/p775/x86-boot-tests-ab.md`.
All six runs' runlogs and user serials are now committed under
`docs/planning/green-program/sockets/serials/787-regression/ab-serials/`; round 2
added `pre-2` and `pre-3`, which round 1 cited but did not commit.

| leg | run | gate verdict | cohort `START`->`PASS` | 1-min load at launch |
|---|---|---|---|---|
| pre `ee6de882` (merge of PR #785) | pre-1 | FAIL | cleared, 80->221 s = 141 s | 1.89 |
| | pre-2 | PASS | cleared, 65->196 s = 131 s | 1.15 |
| | pre-3 | PASS | cleared, 65->196 s = 131 s | 1.55 |
| main `b257e69e` (merge of PR #787) | main-1 | FAIL | never reached `PASS`; wedged at child 49 of 64 | 1.10 |
| | main-2 | FAIL | never reached `PASS`; wedged at child 13 of 64 | 1.09 |
| | main-3 | FAIL | never reached `PASS`; wedged at child 14 of 64 | 0.89 |

claim-lint:ok: every cell is read back out of `ab-serials/<run>.runlog` --
`cohort_start_elapsed_s`, `cohort_end_elapsed_s`, `load_before` and the
`verdict_grep` line, 6 of 6.

pre-1 is a FAIL, and round 1's table hid that by reporting only the cohort
number. The failure is NOT the wedge: pre-1's cohort cleared normally at
elapsed 221 s and the boot went on to `[TEST:userspace:loopback_recv_wake:PASS]`,
the same last marker pre-2 and pre-3 reach. The gate then ran out of its
900-iteration poll budget waiting for `RECLAIM_DRAIN` /`TOMBSTONE_QUIESCE` /
`KSTACK_QUIESCE_LEAK`, which never appeared, and aborted at
`run-x86-boot-tests.sh:682` with the boot still progressing. So the pre leg is
2 of 3 green overall and 3 of 3 on the cohort specifically, and the honest
statement of the A/B is about the cohort, not about the whole gate.

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
   can run again to resume the holder. The CPU spins in the accessor; no further
   byte is printed.
   claim-lint:ok: IF=0 is the `eflags 0x2` reading in both specimen registers
   dumps, `serials/787-regression/spec{1,2}/gdb_sample_1.txt`, 2 of 2.

## Which dispatch arm reaches the lock, and what the markers do and do not show

The consumer in step 1 is only reached for dispatches that go through
`setup_kernel_thread_return`. In `switch_to_thread` the idle thread reaches it
only on the `<1>` arm (`context_switch.rs:881`, idle WITH a saved context); the
`<I>` arm (`context_switch.rs:887`, `setup_idle_return`) does not touch the
master PML4 at all. That much is structural and is what the specimens land on.

### The predicate, in full

`has_saved_context` is the conjunction of two terms, not one
(`kernel/src/interrupts/context_switch.rs:864-876` at this head):

1. `crate::syscall::handler::is_ring3_confirmed()`. Once userspace has started,
   `has_saved_context` is set to `false` unconditionally and the second term is
   not evaluated, so an idle dispatch after that latch takes `<I>`. The
   comment above it gives the reason: idle's boot-time saved context can hold
   RIPs in kernel init code that hang when restored during userspace operation.
   claim-lint:ok: the short-circuit is the `if userspace_started { false }` arm,
   1 of 1, at `context_switch.rs:866-868`.
2. Only while ring 3 is NOT yet confirmed: `thread.context.rip != 0 &&
   thread.context.rip != idle_loop`, i.e. some save has written a RIP that is
   neither `0` nor the idle entry.
   claim-lint:ok: the 2 of 2 tested values are `0` and
   `idle_loop as *const () as u64` at `context_switch.rs:871-875`.

Round 1 described term 2 only. Term 1 dominates -- it is a boot-phase latch, and
it is the term that decides the arm for the post-userspace part of a boot -- so
omitting it made the arm look like a per-thread property when it is mostly a
phase property.
claim-lint:ok: both terms are the `has_saved_context` binding at
`context_switch.rs:864-876`, consumed by `if has_saved_context` at `:878`;
term 1 is evaluated first, 1 of 1.

### The marker counts, and why they carry no frequency story

Round 1 put a four-row table here contrasting 461 `<I>` on the pre leg with 0
`<I>` on main, "over each whole boot", and read it as the mechanism by which
#787 moved the master-PML4 read onto the dispatch path. That contrast is a
truncation artifact, and this round retracts it.

What the committed serials actually measure:

| serial | bytes | whole boot `<I>`/`<1>`/`<K>` | before cohort `START` | in the cohort window |
|---|---|---|---|---|
| `ab-serials/pre-1` (`ee6de882`, PASS at cohort) | 68048 | 461 / 23 / 54 | 0 / 1 / 2 | 0 / 0 / 0 |
| `ab-serials/pre-2` (`ee6de882`, PASS) | 67155 | 468 / 23 / 53 | 0 / 1 / 2 | 0 / 0 / 0 |
| `ab-serials/pre-3` (`ee6de882`, PASS) | 56504 | **3** / 23 / 53 | 0 / 1 / 2 | 0 / 0 / 0 |
| `ab-serials/main-1` (`b257e69e`, wedged) | 4296 | 0 / 59 / 62 | 0 / 12 / 14 | 0 / 47 / 48 |
| `prove/single-1` (fixed, PASS) | 72239 | 402 / 320 / 391 | 0 / 12 / 14 | 0 / 97 / 97 |

Three facts kill the causal reading:

- **The 461-vs-0 gap is length, not leg.** A wedged run's user serial stops at
  ~4 KB; a run that clears the cohort goes on to ~70 KB. pre-1's *first* `<I>`
  is at byte 55917 of 68048 -- 52 KB after the cohort passed, in the
  `[TEST:userspace:loopback_recv_wake]` phase. The wedged legs never reached
  that phase, so they could not have printed an `<I>` whatever the dispatch
  shape was. Both legs are 0 `<I>` everywhere the two are comparable.
- **The pre leg disagrees with itself by 150x.** Same commit, same binary, same
  131 s cohort, same last test marker reached: pre-1 461, pre-2 468, pre-3 3.
  A count that varies that much across three runs of one binary is measuring
  how long the boot sat in its trailing idle stream, not a property of the tree.
- **The fixed, green head has 402 `<I>` whole-boot.** If a low `<I>` count were
  the wedge condition, the fix's own passing runs would contradict it.

What the serials DO support, stated no wider than that: in the cohort window
the pre leg emitted **no traced dispatch at all** -- START and PASS are adjacent
lines, 39 bytes apart -- while both the wedging `b257e69e` and the fixed head
emit a dense alternating `<K>`/`<1>` stream there (47/48 and 97/97). The pre
leg's window is silent, so it cannot be compared arm-for-arm with the other two
at all, and the round-1 sentence that #787 "moves the master-PML4 read from
rarely on the dispatch path to on essentially every idle dispatch" is not
established by anything committed here.
claim-lint:ok: every number in the table is re-derivable from the named file
with `grep -o` and a byte offset; the window bounds are the byte offsets of
`[TEST:process:x86_retire_cohort:START]` and `:PASS]` in the same file.

**The frequency story is not established.** Neither the per-run probability on
each leg, nor the attribution of any change in dispatch-arm mix to #787's
`blocked_in_syscall = owner_pid.is_some()` producer fix, nor why the branch head
`18d35cac` measured 18/20 rather than 0/20, is supported by the evidence in this
record. Establishing it would take an instrumented A/B that counts arms inside
the cohort window on both legs, which was not run.

This retraction does not weaken the finding. The mechanism rests on the two GDB
specimens below -- IF=0, spinning in the master-PML4 accessor, on the dispatch
stack, with the lock byte read as 1 -- and on the source-level fact that ordinary
thread context takes the same mutex with interrupts enabled. The fix rests on its
own green battery. The dropped claim was decoration on both.

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
   accessor call. That removes this deadlock at its source: the master PML4 read
   on the dispatch path is now a plain atomic load.
   claim-lint:ok: the 3 code lines that name the cell are pinned by
   `tests/dispatch_path_lock_free_structure.rs`.

   The dispatch path is NOT lock-free after this change, and round 1 said it was
   ("the dispatch path has no lock left to spin on", in this document and in
   commit `6d17b83a`'s message; corrected in the PR body, since the message is
   pushed). `setup_kernel_thread_return` still calls
   `scheduler::with_thread_mut` (`context_switch.rs:1293`), which takes
   `SCHEDULER.lock()` -- also a `spin::Mutex` -- via `lock_scheduler`
   (`scheduler.rs:4719-4728` -> `:330-334`). What keeps that acquisition out of
   the failure class of step 6 is not its absence but the interrupt state of its
   holders: `with_thread_mut` wraps the acquire and the whole critical section in
   `without_interrupts` (`scheduler.rs:4723`), so a holder cannot be preempted
   while holding it, and the byte the dispatch spins on is owned by a holder that
   is still on CPU. The master-PML4 readers had the opposite property --
   `map_kernel_page` took its mutex from ordinary thread context with interrupts
   ENABLED -- and that is the whole difference.
   Narrowed deliberately: this round checked `with_thread_mut`, the one
   acquisition the dispatch path uses. `SCHEDULER` has 30 acquisition sites in
   `scheduler.rs`; 1 of those 30 was audited for the masked-holder property here,
   so this is a statement about the dispatch path's own call, not a
   whole-lock invariant.
   claim-lint:ok: the site count is `grep -c "lock_scheduler()"
   kernel/src/task/scheduler.rs` at this head; the audited site is
   `scheduler.rs:4723`, 1 of 30.
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

## Proof, round 1: 6 of 6 single gate runs, 8 of 8 four-boot runs, 3 of 3 production runs

Plus the ratchet mutation at the end of this section. Round 2's re-smoke at the
merged head, including the aarch64 leg, is in its own section below.

The runs are on beast, in the `breenix-x86` Incus container, sharing the machine
with the `/root/breenix-737-oracle` tenant throughout. Gate stdout is committed
under `docs/planning/green-program/sockets/serials/787-regression/prove/`.

x86, `docker/qemu/run-x86-boot-tests.sh 1`, six sequential runs -- the gate that
was red 3 of 3 on `b257e69e` and 5 of 5 across the two specimens: 6 of 6 print
`x86 frame-custody gate run 1: PASS`, one `single-N.stdout` per run under
`docs/planning/green-program/sockets/serials/787-regression/prove/`.
`prove/single-1/` carries that run's full serial pair, and its
`[TEST:process:x86_retire_cohort:PASS]` is the marker the wedging legs never
reached.

Round 1 also printed a per-run wall-clock table here -- six `START`->`PASS`
durations and six totals -- and a "127-133 s band that sits inside the pre-#787
leg's own 131-141 s". Both are withdrawn:

- The seconds came from a wrapper (`battery1.sh`) that printed them to a
  terminal only; the committed `single-N.stdout` files carry no timestamps, so
  no committed artifact backs those twelve numbers. They are deleted rather than
  restated.
- The band sentence was arithmetically false in any case: 127-133 does not sit
  inside 131-141, it sits below it at the low end. And the comparison was never
  like-for-like -- the pre leg ran at 1-min loads of 1.89 / 1.15 / 1.55 with the
  `/root/breenix-737` tenant active, and the fix battery ran alongside
  `/root/breenix-737-oracle`, on a host whose load moved run to run. On this
  machine a cohort duration is a load reading as much as a code reading.

What the committed stdouts do support is the verdict and the counts, which is
what this section now claims.

x86, `docker/qemu/run-x86-boot-tests.sh 4`, two batches: 4 of 4 PASS in each,
8 of 8 overall, in
`docs/planning/green-program/sockets/serials/787-regression/prove/gate4-1.stdout`
and `gate4-2.stdout`. (Round 1's "batch 1 took 1837 s wall" came from the same
unsaved wrapper output and is deleted.)

x86 production profile, `docker/qemu/run-x86-prod-profile-boot-test.sh`, 3 of 3
runs exit 0, at 107 s / 109 s / 108 s, in
`docs/planning/green-program/sockets/serials/787-regression/prove/prod-1.stdout`
and its two siblings.
claim-lint:ok: the three `rc=0` lines and the three `total_s` values are the
committed `prove/prod-summary.txt`, which round 2 copied off beast for exactly
this reason; 3 of 3.
That script builds with no `--features` at all, so it is
also the zero-feature build check; the
`boot_tests,testing,external_test_bins` build printed no `warning`/`error`
line when re-run in that slot (`grep -E "^(warning|error)"` -> no output).

aarch64: round 1 claimed "80 of 80 aarch64 strict boots" from four 20-boot
invocations. No log of those runs was committed and none was found on this Mac
when round 2 went looking, so the claim is withdrawn and replaced by the
three committed strict boots in the round-2 section below. The soft-float
`--features boot_tests` build being clean is likewise re-measured there rather
than asserted from round 1.

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

## Round 2 (R161)

Round 2 was a review round. It corrected six claims this record made that the
committed serials do not support -- the marker table (F1), the `<I>` predicate
(F2), the cohort-duration band (F3), the wall-clock numbers (F4), the A/B pre
row (F5) and "no lock left to spin on" (F7) -- and it rebuilt the allocation
ratchet (F9). Each correction is written into the section it belongs to above
rather than collected here; this section carries only what round 2 MEASURED.

The branch was merged with `origin/main` at `bdb5be90` (PR #795) with no
conflict before any of it ran, so the numbers below are from the merged head.
claim-lint:ok: the merge commit is on this branch and `git merge-base
--is-ancestor origin/main HEAD` succeeds, 1 of 1.

### The allocation ratchet, and the vacuous guard round 2 shipped first

Round 2's first attempt at a binary-level allocation guard read instruction text
only. Run against a kernel with `thread.name.clone()` restored, it PASSED. The
reason is structural: this kernel is a static PIE and rustc emits the call as
`movq <slot>(%rip), %rax; callq *%rax` through a GOT slot, so the callee's name
is in the relocation, not at the call site. Measured: the restored clone reaches
`<alloc::string::String as Clone>::clone` at `0x3cae80` through slot
`0x4161d0`, whose `R_X86_64_RELATIVE` entry carries exactly that address.

The shipped guard resolves that. It builds three tables -- `.text` function
symbols, `R_X86_64_RELATIVE` relocations, the disassembly -- and resolves each
in-scope instruction's target directly (`callq ... <SYM>`) or through the
relocation. Four legs, each committed under
`docs/planning/green-program/sockets/serials/787-regression/round2/alloc-guard/`:

| leg | ELF sha256 (first 8) | result |
|---|---|---|
| fixed kernel, `boot_tests` profile | `bd255118` | PASS, 3 symbols, 19 edges, 0 violations |
| fixed kernel, production profile | `c2920c61` | PASS, 3 symbols, 19 edges, 0 violations |
| `name.clone()` restored to its `b257e69e` form | `a4929756` | FAIL, 3 violations |
| the SHIPPED `b257e69e` kernel that wedged the gate | `d3e2fa94` | FAIL, 3 violations |

The three violations are the same in both red legs: the closure's
`String::clone` and two `String` drop-glue calls. The fourth leg is the one
worth reading -- it is not a synthetic mutation but the binary that actually
wedged, so the allocation this fix removed is now confirmed present in the
shipped artifact and not only in the source.

Two anti-vacuity arms: the guard fails when it finds no in-scope symbol, and
when it resolves no call edge. Depth is 1, deliberately: a transitive walk from
this function reaches `log::error!`'s formatting machinery on the else arm and
would redden a clean tree. The disclosed blind spot is an allocation two frames
down behind a callee that is not itself an alloc-crate symbol.

The source-level assertion is now described as what it is -- a denylist, widened
from 3 spellings to 13 -- in its own module header, in the fix section above, and
in the PR body.

### x86, at the merged head, on beast

`/root/breenix-787fix` at `cd17ff25`, in the `breenix-x86` Incus container,
sharing the machine with other tenants. `pgrep -fl qemu-system-x86_64 | wc -l`
was recorded immediately before each boot: 0, 0, 0.

- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
  piped through `grep -E "^(warning|error)"` produced a 0-byte file
  (`round2/x86/build-grep.txt`).
- `docker/qemu/run-x86-boot-tests.sh 1`, twice: `x86 frame-custody gate run 1:
  PASS` both times, `rc=0`, and one `[TEST:process:x86_retire_cohort:PASS]` in
  each run's user serial -- the marker absent from 3 of 3 wedging main legs. The
  allocation guard ran inline in both, after the build, reporting 3 symbols and
  19 edges with 0 violations.
- `docker/qemu/run-x86-prod-profile-boot-test.sh`, once: `rc=0`.

Both boot-tests runs booted the same kernel ELF, sha256
`bd255118bd5948d4451a6405c6ad1dff8a025960f5345b18c2a47617e8708e50`; the
per-run UEFI image, test-binary and ext2 hashes are in
`round2/x86/boot-{1,2}.hashes` and `round2/x86/prod-1.hashes`. Full stdout,
both user serials and run 1's kernel serial are under `round2/x86/`.

### aarch64, at the merged head, on this Mac

`pgrep -fl qemu-system-aarch64 | wc -l` was 0 before each of the three launches.

- The soft-float `boot_tests` build completes. It is not silent: it prints one
  `warning:` line, the future-incompatibility notice for the toolchain's own
  vendored `core v0.0.0`, which names no Breenix crate. Round 1 called this
  build "clean" without that qualification.
  claim-lint:ok: the line is the last of `round2/a64/build-tail.txt`, 1 of 1.
- `scripts/check-kernel-no-neon.sh`: PASS, 0 FP/SIMD load/store instructions in
  kernel `.text` (`round2/a64/no-neon.txt`).
- `docker/qemu/run-aarch64-boot-test-strict.sh 1`, three times, one at a time:
  `PASS: 1/1 boots succeeded` each (`round2/a64/strict-{1,2,3}.txt`), 3 of 3.

This replaces round 1's withdrawn "80 of 80". Three boots is what round 2 ran and
three is what it claims.

### Structure suites

All 28 `tests/*_structure.rs` suites were run once at this head: 28 green of 28,
including the widened `dispatch_path_lock_free_structure` at 4 of 4
(`round2/structure-suites.txt`).

### claim-lint

```
claim-lint: python3 scripts/claim-lint.py                        -> exit 0
claim-lint: python3 scripts/claim-lint.py --files <pr body .md>  -> exit 0
```

Each of round 2's four content commits -- `ae924cdb`, `ea3451f4`, `cd17ff25`,
`4c5fd2d6` -- ends with the first of those lines. The round's fifth commit, the
`origin/main` merge `2bc6663d`, does NOT: it was written with `git commit -m`
and carries no claim-lint line. It is pushed, so it is disclosed here rather
than rewritten. It introduces no authored prose of its own -- `git show --stat`
on it lists only what main brought in -- and the round's claim-lint record is
this block plus the four commits that do carry it.
