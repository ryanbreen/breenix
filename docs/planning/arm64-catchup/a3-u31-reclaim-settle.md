# A.3 U31: settle through receipt completion

Coordinator ledger lines 288–289 govern this unit. It performs source work
and host checks without a boot. The earlier captures are
`20261112T051028Z-qemu-76740` (55 retained pages at `c48b19b9`) and
`20261112T061527Z-qemu-21504` (10 retained pages at `756d461c`). Both after
records report `receipts=0` and settle for about 202–205 ms. Neither capture
reports the detached receipt state, so those records alone cannot establish
that the interleaving below occurred in the failed boot.
<!-- claim-lint:ok: retained Vigil serial MEMORY records; first excerpt also preserved in serials/a3-memory-c48b19b9.json. -->

## Source decision

The settle already waits for queued and parked ARM exit receipts. Those are
the same receipts that own the deferred leaves and tables, rather than a
separate pipeline invisible to the reader:

- `kernel/src/task/process_task.rs::handle_thread_exit` calls
  `defer_remaining_tables` for ARM normal exit, then enqueues its contents
  outside PM. `defer_remaining_tables` calls `defer_process_resources`, which
  moves the current and superseded roots into `PendingProcessReclaim`.
- `/proc/breenix/frames` calls `reclaim_queue_lengths`, reporting the pending
  and parked vectors' lengths. `suite_memory.rs::read` sums those lengths,
  and `settled` requires receipts=0 in two readings with equal free-page counts.
  <!-- claim-lint:ok: baseline756d461c suite_memory.rs read sums pending+parked; settled compares both receipt counts to 0. -->
- The drain's `reclaim_deferred_process_resources_for_pass` removes a ready
  receipt with `pending.swap_remove(index)`, releases that queue lock, and
  then runs root proofs and `reclaim_bounded`. An incomplete bounded step is
  requeued; a completed one calls `manager.retire_row`.

Thus **queued work was covered, but the pipeline was not covered through
completion**. During the detached interval, both queues can be empty while
the receipt still owns the exited process's user leaves and tables. The
publication interval is also absent from those vector lengths: the row is
marked terminated before phase 2 enqueues its receipt. No change to a reclaim
budget or scheduling policy is needed to repair the observer's omission.
<!-- claim-lint:ok: baseline756d461c process_task.rs632-640,698-729,1101-1114,1224-1227,1505-1510,1965-2039; procfs/mod.rs1241-1259; suite_memory.rs127-129,169-176. -->

## Change

Expose the existing `Process::retirement_receipts` count through
`outstanding_retirement_receipts`. `ProcessManager` sums it over the resident
map, including reaped tombstones; its ordinary `iter_processes` deliberately
excludes tombstones and is unsuitable here. The proc endpoint exports
`reclaim outstanding=<n>`. The suite requires that row to parse and waits for
outstanding=0 in both stable readings, as well as its existing
queue and free-count conditions.
<!-- claim-lint:ok: process.rs and manager.rs outstanding_retirement_receipts; procfs/mod.rs generate_frames; suite_memory.rs read/settled. -->

The count already spans the resource lifetime: `defer_process_resources` and
`defer_superseded_roots` call `note_receipt_created` before handing out a
receipt; the completed drain calls `retire_row`, which settles the count.
The reap/retire join retains the row while its resources are owed. Reading
that count covers publishing, queued, parked and drain-owned receipts without
introducing another lifetime counter or changing the reclaim path.
<!-- claim-lint:ok: process_task.rs defer_process_resources, defer_superseded_roots and completed drain; process.rs note_receipt_created/settle_retirement_receipt; manager.rs outstanding_retirement_receipts, retire_row, remove_row_joined. -->

The 48-page allowance, 100 ms stable interval and five-second reclamation
watchdog remain unchanged. The existing timeout still returns the last
reading. The serial MEMORY record retains its existing field set; the new
count lives in the proc census. No Tier-1 or Tier-2 file changes.
Both bounded ownership snapshots now export after the verdict on PASS as
well as FAIL. Their capture boundaries and pre-observer ordering are
unchanged; incomplete or unavailable snapshots remain marked as such.
<!-- claim-lint:ok: suite_memory.rs constants, read, settled and record; suite.rs verdict/export block; diff contains no Tier-1/Tier-2 path. -->

## Deterministic evidence

The check-first commit is `4be60905`. On `756d461c`, the production reader and
settle loop were run with a virtual clock and proc-file boundary fixture:
an exited process owns 44 user pages and 12 table pages; its receipt is queued
until 300 ms, then owned by the drain until 600 ms. The queued-receipt guard
passed, but settling returned at 400 ms with free=944 of 1000 and receipts=0.
The assertion requiring those 56 pages back failed. This forces the missing
state rather than relying on host scheduling or repeating a guest boot.
The separate ownership-export check failed because export was conditional
on a failed verdict.
<!-- claim-lint:ok: /tmp/a3-u31-red.log and /tmp/a3-u31-export-red.log retain both failures; tests/suite_memory_reclaim_settle_test.py and tests/memory_ownership_snapshot_test.py. -->

After implementation, the fixture also compiles the production per-row
create/read/settle methods and the manager's census method. The stable sample
waits until at least 700 ms, after completion and one stable interval, and
returns free=1000. The ownership check requires both exports before the
failure-only detail branch. Both checks pass. The production Rust fixture
compiles with `-Dwarnings`.
<!-- claim-lint:ok: /tmp/a3-u31-green.log and /tmp/a3-u31-export-green.log; suite_collection_test.py run_rust; the assertion checks clock>=700 and free1000. -->

## Validation and handoff

Release build results, with `CARGO_BUILD_JOBS=4` and background build policy:

| Target | Scope | Result |
| --- | --- | --- |
| ARM64 | Production `kernel-aarch64` | Exit 0, 1m33s |
| x86 | `qemu-uefi`, `testing,external_test_bins` | Exit 0, 1m59s |
| ARM64 | `suite-window-manager` | Exit 0, 18.36s |
| x86 | `suite-window-manager`, `framebuffer-v1` | Exit 0, 19.41s |

The builds emitted no new source warnings. ARM's kernel build repeated the
pinned-core future-incompatibility advisory retained in the preceding unit's
build log. Suite builds used the verified Rust fork and its canonical library
path, with the same compiler metadata and link flags as the userspace build
script. Local logs are `/tmp/a3-u31-{arm,x86}-{kernel,suite}.log`.
<!-- claim-lint:ok: four build logs above, compared with /tmp/a3-u30-arm-kernel.log; scripts/rust-fork.sh verify reports666252fd61839b9e9b1bc9e777a0c84c5cf3dc6d. -->

Initial changed-hunk claim-lint (`--base 756d461c`) exited 0. Report lint
initially found two source claims requiring local evidence; the paragraphs
now name the source expressions and whole-file report lint exits 0. See the
effort ledger for final default claim-lint and pushed-head records.
<!-- claim-lint:ok: /tmp/a3-u31-* host/build outputs and this session's claim-lint invocations; no expectation or threshold edited. -->

No boot, debugger, PR creation or merge occurs in this unit. Issue #2872's
boot attribution and the later acceptance unit remain outstanding. That unit
owns the coordinator's one ARM proof and one native window-manager boot;
these host checks and builds do not claim that acceptance.
