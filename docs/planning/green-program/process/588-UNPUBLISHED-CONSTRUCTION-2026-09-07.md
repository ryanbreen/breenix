# 588: unpublished-construction ownership in the process builders

Round date: 2026-09-07. Branch: process/588-unpublished-construction.
Issue: 588 (process creation failure after page-table construction leaves a
counted, never-freed frame residue).

## What changed

Issue 588 reports that the process builders construct the address space first
and only then run the fallible work that fills it, so a failure anywhere below
the construction drops the page table with `Disposition::Undecided`. The Drop
impl in `kernel/src/memory/process_memory.rs` counts that drop
(`PT_ROOT_DROPPED_UNDECIDED`) and returns no frames.

This round gives the builders ownership across the complete fallible
construction, in two halves with no fallible step between them:

* `UnpublishedPageTable` owns the boxed table from construction until it moves
  into a local `Process`. This is the carrier exec already uses.
* `UnpublishedProcess` (new, `kernel/src/process/unpublished.rs`) owns the
  partially-built `Process` from that point until the row insert. Its Drop
  hands the table to `UnpublishedPageTable::from_box` (new, added so the
  transfer does not unbox and reallocate the table), which runs the same
  release: `release_mapped_leaves` then `retire_bounded`. `commit` is the
  only way out and disarms the release, so a table that reaches a row is not
  released here.

`from_box` keeps the release on the single `retire_bounded` call
site the R3 census pins, so the guard adds no new retirement site.
claim-lint:ok: issue 588; serials under docs/planning/green-program/process/serials/588/

## Boundary table

The 40 rows below are the fallible boundaries between page-table construction
and the row insert, with the guard that owns the address space if one is taken.
Path 1 and 2 are x86, path 3 and 4 are aarch64; the four builders live in
`kernel/src/process/manager.rs`.
claim-lint:ok: issue 588; serials under docs/planning/green-program/process/serials/588/

| path: boundary | cleanup owner |
| --- | --- |
| build_process_at (x86): ProcessPageTable::new map_err | no table exists yet |
| build_process_at (x86): load_elf_into_page_table | UnpublishedPageTable |
| build_process_at (x86): map_page in the kernel low-half restore | UnpublishedPageTable |
| build_process_at (x86): translate_page verification of the restore | UnpublishedPageTable |
| build_process_at (x86): allocate_stack_with_privilege | UnpublishedProcess |
| build_process_at (x86): map_user_stack_to_process | UnpublishedProcess |
| build_process_at (x86): missing page table for stack mapping | UnpublishedProcess |
| build_process_at (x86): create_main_thread | UnpublishedProcess |
| build_process_at (x86): row insert | terminal, guard committed |
| build_process_with_argv_at (x86): ProcessPageTable::new map_err | no table exists yet |
| build_process_with_argv_at (x86): load_elf_into_page_table | UnpublishedPageTable |
| build_process_with_argv_at (x86): map_page in the kernel low-half restore | UnpublishedPageTable |
| build_process_with_argv_at (x86): translate_page verification of the restore | UnpublishedPageTable |
| build_process_with_argv_at (x86): allocate_stack_with_privilege | UnpublishedProcess |
| build_process_with_argv_at (x86): map_user_stack_to_process | UnpublishedProcess |
| build_process_with_argv_at (x86): missing page table for stack mapping | UnpublishedProcess |
| build_process_with_argv_at (x86): setup_argv_on_stack | UnpublishedProcess |
| build_process_with_argv_at (x86): missing page table for argv setup | UnpublishedProcess |
| build_process_with_argv_at (x86): create_main_thread_with_sp | UnpublishedProcess |
| build_process_with_argv_at (x86): row insert | terminal, guard committed |
| create_process (aarch64): ProcessPageTable::new map_err | no table exists yet |
| create_process (aarch64): load_elf_into_page_table | UnpublishedPageTable |
| create_process (aarch64): allocate_stack_with_privilege | UnpublishedProcess |
| create_process (aarch64): stack frames unavailable (ok_or) | UnpublishedProcess |
| create_process (aarch64): map_user_stack_to_process_with_phys | UnpublishedProcess |
| create_process (aarch64): map_initial_arm64_tls | UnpublishedProcess |
| create_process (aarch64): missing page table for stack mapping | UnpublishedProcess |
| create_process (aarch64): create_main_thread | UnpublishedProcess |
| create_process (aarch64): row insert | terminal, guard committed |
| build_process_with_argv_at (aarch64): ProcessPageTable::new map_err | no table exists yet |
| build_process_with_argv_at (aarch64): load_elf_into_page_table | UnpublishedPageTable |
| build_process_with_argv_at (aarch64): allocate_stack_with_privilege | UnpublishedProcess |
| build_process_with_argv_at (aarch64): stack frames unavailable (ok_or) | UnpublishedProcess |
| build_process_with_argv_at (aarch64): map_user_stack_to_process_with_phys | UnpublishedProcess |
| build_process_with_argv_at (aarch64): map_initial_arm64_tls | UnpublishedProcess |
| build_process_with_argv_at (aarch64): missing page table for stack mapping | UnpublishedProcess |
| build_process_with_argv_at (aarch64): setup_argv_on_stack | UnpublishedProcess |
| build_process_with_argv_at (aarch64): missing page table for argv setup | UnpublishedProcess |
| build_process_with_argv_at (aarch64): create_main_thread_with_sp | UnpublishedProcess |
| build_process_with_argv_at (aarch64): row insert | terminal, guard committed |

The row insert is terminal for the four builders themselves, but not for the
two init entry points that call one of them: `create_init_process_at` (x86,
`kernel/src/process/manager.rs:589-596`) and
`create_init_process_with_argv_at` (aarch64, same file, 1270-1279) each run
`build_process_at`/`build_process_with_argv_at`, then `?`, then
`hold_init_publication`. `hold_init_publication`'s `init row has no main
thread` arm (`manager.rs:241-245`) calls `drop(self.remove_process(provisional_pid))`;
`remove_process` (`manager.rs:1845-1847`) is `self.take_row_unconditionally(pid)` --
it returns the row rather than dropping it internally (this changed shape
came in on the R16 merge; the drop itself still happens, just at the call
site in `hold_init_publication` instead of inside `remove_process`) -- and
that returned row's page table then hits `Drop for ProcessPageTable` with
`Disposition::Undecided` (`kernel/src/memory/process_memory.rs:2022-2027`) --
issue 588's exact defect, one step past this table's terminal row. Not
reachable today: `set_main_thread` precedes `processes.insert` in all 4 of 4
image-loading builders (`manager.rs:573`/584, 787/796, 1037/1051, 1252/1265),
so the `None` arm `hold_init_publication` checks for cannot fire from these
callers, and no oracle in this round drives it. The table above is scoped to
the four builders' own fallible steps and does not cover this caller-side
step.
claim-lint:ok: #588; manager.rs:241-245,589-596,1845-1847,1270-1279; process_memory.rs:2022-2027

## Why the stack frames are not freed by the release

`release_mapped_leaves` only releases leaves it holds a custody record for, and
it classifies each one through `acquire_leaf_mapping`. Both architectures
register user stack frames as external leaf frames in
`allocate_stack_with_privilege` (`kernel/src/memory/stack.rs`, the
`register_external_leaf_frame` call on the `ThreadPrivilege::User` arm), so the
release drops the mapping and returns no frame. The stack owner keeps whatever
custody it had. Issue 583 still describes the `GuardedStack::drop` leak; this
round does not change it either way.
claim-lint:ok: issue 588; serials under docs/planning/green-program/process/serials/588/

## Oracle

`init_designation_oracle_test` (`kernel/src/tracing/providers/teardown.rs`)
already drove two construction failures per boot and measured the frame delta
across them. This round makes it state exactness rather than a nonnegative
bound, and adds a publication leg.

Marker fields added: `construct_roots_retired`, `construct_leaf_balance`,
`construct_commit_balance`. In-kernel requirements over the two-failure
window: `construct_failed == 2`, `construct_undecided == 0`,
`construct_residual == 0`, `construct_roots_retired == 2`, leaves released
== leaves recorded, leaf frames returned == leaves recorded, table frames
returned == table frames recorded + 2 (the two roots), no mid-retire drop, no
lost frame return, and a 0 delta on each of the 6 refusal counters (live-leaf,
double, stale, untracked, leaf-custody, root-slot). The publication leg
(`commit_preserves_live_table_for_gate`) constructs a table, hands it through
`UnpublishedProcess::commit`, requires the used-frame count unchanged across
the commit, then releases the committed table and requires the count back at
its pre-construction value.
claim-lint:ok: issue 588; serials under docs/planning/green-program/process/serials/588/

Pinned literals were re-derived from measured runs, not assumed, in
`docker/qemu/run-x86-boot-tests.sh`,
`docker/qemu/run-aarch64-full-test.sh` and
`docker/qemu/run-aarch64-boot-test-strict.sh`. The recorded-serial fixture
`tests/fixtures/udp-socket-lock-aarch64-serial.txt` carries the same marker line
because four suites replay it through the strict gate scorer; its replacement
line is copied byte-for-byte from the measured aarch64 boot below, not written
by hand.

### Red line (fix reverted, oracle kept)

With `kernel/src/process/manager.rs` restored to origin/main 1ee30c10269f and
the rest of the branch in place, the aarch64 native boot test reddens:

    [INIT_DESIGNATION_ORACLE:aarch64:construct_failed=2:construct_undecided=2:construct_residual=2:construct_roots_retired=0:construct_leaf_balance=0:construct_commit_balance=0:refused=4:accepted=1:published=1:retired=1:held_error_removals=1:reparented=1:reparent_skipped=1:ordinary_allocated=5:reserved_collisions=0:designation_balance=0]
    [TEST:process:init_designation_oracle:FAIL:failed construction dropped a page table Undecided]

Serial: `docs/planning/green-program/process/serials/588/red-aarch64-native-main-manager.txt`

### Green line (branch HEAD)

    [INIT_DESIGNATION_ORACLE:aarch64:construct_failed=2:construct_undecided=0:construct_residual=0:construct_roots_retired=2:construct_leaf_balance=0:construct_commit_balance=0:refused=4:accepted=1:published=1:retired=1:held_error_removals=1:reparented=1:reparent_skipped=1:ordinary_allocated=5:reserved_collisions=0:designation_balance=0]
    [TEST:process:init_designation_oracle:PASS]

Serial: `docs/planning/green-program/process/serials/588/green-aarch64-strict-boot1.txt`

## Structure ratchet

`fallible_process_construction_holds_unpublished_ownership` in
`tests/teardown_structure.rs`. The census is shape-derived: a builder is a
function in the process manager that loads an ELF into a page table and inserts
a process row, which resolves to four functions. Removing a guard does not
remove a function from that census, so the mutations redden rather than
vacuously pass. Per builder it requires one page-table construction wrapped by
one unpublished carrier, one publish before one guard arming, a row insert whose
argument is the guard commit, and at least one fallible step inside each of the
two spans.

Mutations run in the same test, each reddening on unmodified source plus the one
change:

* `UnpublishedProcess::new(` to `core::convert::identity(`, first occurrence:
  `build_process_at does not arm exactly one unpublished-construction guard`
* `UnpublishedPageTable::new(` to `alloc::boxed::Box::new(`, first occurrence:
  `build_process_at does not wrap its one page-table construction in an unpublished carrier`
* the row insert argument changed off `unpublished.commit()`:
  `build_process_at does not insert its row through the guard commit`

Three existing pins moved with the change and are recorded here so a reviewer
can see they were not loosened: the F2 terminality mutations in
`init_row_builders_insert_only_after_all_fallible_steps` now quote the committed
insert; the `PROCESS_PAGE_TABLE_CONSTRUCTORS` census gains the new gate leg; and
the `missing_exec_carrier` loop in `deliberately_broken_variants_fail_the_ratchet`
moves from n in 0..4 to n in 4..8, because the four builders now also contain
`UnpublishedPageTable::new(` and come first in file order.

## Gates

| gate | bytes | result |
| --- | --- | --- |
| scripts/run-structure-tests.sh, each of the 62 tests/*_structure.rs files | branch HEAD, Mac | 62/62 suites pass |
| docker/qemu/run-aarch64-boot-test-strict.sh 3 | branch HEAD, Mac | 3/3 boots, PASS |
| docker/qemu/run-aarch64-prod-profile-boot-test.sh | branch HEAD, Mac | PASS |
| docker/qemu/run-x86-boot-tests.sh 1 | branch HEAD, beast | PASS on the 4th attempt; the 3 earlier attempts are attributed below |

`docker/qemu/run-aarch64-full-test.sh` also carries the new
`INIT_DESIGNATION_ORACLE` literal (updated to keep its grep in sync), but that
gate was not run this round -- it is not exercised evidence, only a literal
kept from going stale. I diffed it against the strict gate's literal above:
byte-identical.

Logs and serials under `docs/planning/green-program/process/serials/588/`:
* `red-aarch64-native-main-manager.txt`
* `green-aarch64-strict-boot1.txt`
* `green-aarch64-strict-3boots.txt`
* `green-aarch64-prod-profile-serial.txt`
* `green-aarch64-prod-profile-gate.txt`
* `green-x86-boot-tests-serial.txt`
* `green-x86-boot-tests-gate.txt`
* `x86-run1-891-ksoftirqd-panic-serial.txt`

### x86 attempts on beast

The container `breenix-x86` was shared with other lanes for the whole round
(load average 5 to 9.5 on 8 cores). Four attempts ran on the same clone of this
branch at commit 66ef1730:

1. Structure preflight green 62 of 62, boot reached
   `[TEST:process:init_designation_oracle:PASS]` with the pinned x86 marker,
   then panicked later in `kernel/src/task/softirq_tests.rs:228`
   (`ksoftirqd should have processed deferred softirqs`). That is the exact
   signature of issue 891, which records the same assert on this container in
   the same profile and is intermittent there. Preserved serial:
   `x86-run1-891-ksoftirqd-panic-serial.txt`.
2. Structure preflight red, 2 of 62 suites:
   `context_restore_structure` and `gate_capture_drain_structure`.
3. Same 2 suites red again.
4. Structure preflight green 62 of 62, boot green, gate PASS.

The two preflight suites were then run standalone on the same clone at the same
bytes: `gate_capture_drain_structure` 3 of 3 green,
`context_restore_structure` 1 of 1 green (178 s, and its per-suite log in the
red runs shows five tests reporting `has been running for over 60 seconds`).
Both are host-load sensitive under the parallel preflight and neither reads
anything this branch changes; both are green on the Mac at these bytes in the
62 of 62 run above. No x86 red in this round is unattributed.

## Claim-lint

    claim-lint: python3 scripts/claim-lint.py                     -> exit 0
    claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/msg-code.txt -> exit 0
    claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/msg-docs.txt -> exit 0

The round's third commit (79755f9d, the gate-transcript add) went in without
its own recorded line, an omission against the CLAUDE.md "record each
invocation" rule, not a red -- reconstructed here:

    claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/msg-79755f9d.txt -> exit 0

## Not claimed

* The oracle drives two construction failures, and both driven workloads
  (`too_small_image`, `out_of_bounds_segment_image`) fail inside
  `load_elf_into_page_table` before any leaf mapping is recorded, on both
  architectures: the x86 bounds check
  (`if file_start + file_size > data.len()`, `kernel/src/elf.rs:457`) rejects
  the same 120-byte, `p_offset=120`/`p_filesz=1` image `out_of_bounds_segment_image`
  builds for aarch64. `construct_leaf_balance` is `construct_leaf_recorded -
  construct_leaf_returned` (`teardown.rs:5280`); it does not print
  `construct_leaf_recorded` itself, so a marker value of 0 cannot distinguish
  0-of-0 leaves from n-of-n. Both new leaf equalities (`leaves released ==
  leaves recorded`, `leaf frames returned == leaves recorded`) are therefore
  satisfied 0==0 on both architectures and do not establish leaf release on
  either; what moved on aarch64 is the table and root return
  (construct_roots_retired 0 to 2, construct_residual 2 to 0). Both driven
  failures land inside `load_elf_into_page_table`, a boundary this round's own
  table assigns to `UnpublishedPageTable`, the pre-existing exec carrier -- no
  boot in this round executes `UnpublishedProcess::drop`, the new guard,
  since `commit_preserves_live_table_for_gate` (`unpublished.rs:100-125`)
  calls `commit()` and then `UnpublishedPageTable::from_box` directly rather
  than letting an armed guard drop; so the red-to-green oracle delta is
  evidence for the old carrier's exactness, not the new guard's.
  claim-lint:ok: #588; teardown.rs:5280; elf.rs:457; unpublished.rs:100-125
* No failure was injected at the later boundaries (stack allocation, stack
  mapping, TLS, argv, main-thread creation). Those boundaries are covered by the
  structure ratchet and by reading the code, not by an executed error path.
* `fork_process_with_context` in the same file constructs a child page table and
  has fallible steps (`setup_cow_pages_with_vmas`, kernel-stack allocation)
  before its row insert, so it has the same shape as the defect issue 588
  reports. It is not repaired here and is not in the ratchet census, which is
  scoped to the image-loading builders. Filed as issue 932.
* This round does not change the `GuardedStack::drop` leak (issue 583) or the
  x86 user-stack VA bump-allocator exhaustion noted in the manager comments.
* The aarch64 evidence is QEMU only. No Parallels boot was run in this round.

## Fix pass 2026-09-07: E-4, E-9, E-10

A review pass filed three findings against 66ef1730..79755f9d. Each of the
three is closed on this branch, with its own test, on commit d6cef3cd.
claim-lint:ok: #588; tests/teardown_structure.rs

* **E-4** (minor, `tests/teardown_structure.rs:17210`): `validate_unpublished_construction_ownership`
  checked the carrier span (construction to publish) and the guard span
  (guard to insert) for a fallible step, but left the gap from `publishes[0]`
  to `guards[0]` uninspected -- the exact window the module doc
  (`kernel/src/process/unpublished.rs:11`) and the commit message both claim
  is free of one. A `?` inserted between `page_table.publish()` and
  `UnpublishedProcess::new(` would have reopened issue 588 with the ratchet
  green. Fixed by adding a check requiring the span carry no `)?`
  occurrences. claim-lint:ok: #588; tests/teardown_structure.rs
  Test: `fallible_process_construction_holds_unpublished_ownership`
  gained a fourth mutation leg -- `record_publish_watermark(pid)?;` inserted
  right after the first builder's `page_table.publish()` call -- which now
  fails with `build_process_at has a fallible step between the publish and
  the construction guard`.

* **E-9** (nit, `tests/teardown_structure.rs:17153`): the census-failure
  message read "process-builder census resolved fewer than four
  image-loading row builders", but the predicate is `builders.len() == 4`, so
  a fifth builder would also fail while the message named the wrong
  direction. Reworded to "process-builder census did not resolve exactly
  four image-loading row builders". Test:
  `process_builder_census_message_does_not_claim_fewer_when_over_four` builds
  a 5-builder fixture module and asserts the corrected message is present
  and the phrase "fewer than four" is absent from the failure list.

* **E-10** (nit, `kernel/src/memory/process_memory.rs:245`): `from_box`
  (added in 66ef1730) left a doubled blank line above its doc comment and no
  blank line between its closing brace and `publish`. Fixed to match the
  surrounding item spacing. Test:
  `unpublished_page_table_from_box_uses_single_blank_line_separators` pins
  both gaps at exactly one blank line and rejects both the doubled and the
  missing-blank-line shapes.

### Gates (fix pass)

| gate | bytes | result |
| --- | --- | --- |
| scripts/run-structure-tests.sh, each of the 62 tests/*_structure.rs files | d6cef3cd, Mac | 62/62 suites pass; teardown_structure.rs 97/97 tests (was 95/95 before this pass) |
| docker/qemu/run-aarch64-boot-test-strict.sh 1 | d6cef3cd, Mac | PASS 1/1 |
| docker/qemu/run-x86-boot-tests.sh 1 | d6cef3cd, beast /root/breenix-588 | structure preflight 62/62, `x86 frame-custody gate run 1: PASS`, `INIT_DESIGNATION_ORACLE` counters match the prior round's run unchanged (construct_failed=2, construct_roots_retired=2, construct_residual=0) |

New serials under `docs/planning/green-program/process/serials/588/`:
`green-aarch64-strict-fixpass-boot1.txt`, `green-x86-fixpass-gate-run1.txt`,
`green-x86-fixpass-serial-user1.txt`.

### Claim-lint (fix pass)

    claim-lint: python3 scripts/claim-lint.py                          -> exit 0
    claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/msg-code.txt -> exit 0
    claim-lint: python3 scripts/claim-lint.py --commit-msg .tmp/msg-docs.txt -> exit 0

### Not claimed (fix pass)

* The x86 gate ran once (N=1) at this pass's bytes, on a container shared
  with other lanes; the round's earlier x86 evidence (4 attempts, 1 pass, 2
  preflight reds attributed to host load, 1 pre-existing #891 hit) is not
  repeated here and still stands as the multi-attempt record for these
  bytes' behavior under load.
* No Parallels boot was run for this pass; the round's aarch64 evidence
  beyond the 1 strict boot above is unchanged from the prior section.
