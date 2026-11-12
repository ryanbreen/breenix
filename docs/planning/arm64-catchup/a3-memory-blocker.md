# A.3 retained-memory attribution blocker

A.3 remains unproven. This analysis changes no runtime code or acceptance
check, and runs no build or boot. The source is retained ARM capture
`20261112T051028Z-qemu-76740`, clean commit
`c48b19b96022dba6f87d6ee8c0025b40d33b442f`. The extracted records, original
LF line numbers and serial SHA-256 are in
[the evidence excerpt](serials/a3-memory-c48b19b9.json).

The current coordinator ledger was read through line 278. Its last
coordinator ruling is line 273: a failed acceptance is read and stopped for a
ruling. The current brief also explicitly requires a blocker when capture
and code cannot establish the mechanism. No further boot follows this
analysis. <!-- claim-lint:ok: coordinator ledger line 273 and current user stop condition; procedural scope, not an acceptance claim. -->

## What the capture establishes

Serial lines 1275385–1275389 report 81/81 selected cases, then memory
reclamation failure. Line 1280641 reports standalone READY. The memory
boundary is:

| Quantity | Before, line 277 | After, line 1275387 | Change |
| --- | ---: | ---: | ---: |
| Free pages | 84631 | 84576 | -55 |
| User pages | 595 | 639 | +44 |
| Page tables | 11 | 23 | +12 |
| User stacks | 16 | 15 | -1 |
| Buffers | 4218 | 4218 | 0 |
| Other | 130 | 130 | 0 |
| Queued/parked receipts | 0 | 0 | 0 |

Page cache and kernel stacks are both zero at both boundaries. Thus
44 + 12 - 1 = 55 pages, seven pages over the unchanged 48-page allowance.
Both samples settled in 202 ms. `suite_memory::verdict`
(`libs/libbreenix/src/suite_memory.rs:168`) correctly refuses this delta. <!-- claim-lint:ok: serials/a3-memory-c48b19b9.json records contains the two samples and refusal. -->

The later failure-only detail is a separate observation. Lines
1275391–1275403 count 5039 used/allocated frames, zero allocator-lost
frames, 653 user pages and 23 page-table pages. The user count is **14
higher** than the preceding sample's 639. These reads are not atomic and
the observer allocates: `suite_memory::read` retains strings and parsed
owner rows, and `memory_failure_detail` reads another allocated String
(`libs/libbreenix/src/suite_memory.rs:91` and `suite.rs:1614`). The capture
does not establish the individual allocation owners of that difference;
it must not be subtracted from the 55-page verdict as a correction. <!-- claim-lint:ok: serials/a3-memory-c48b19b9.json records preserves both distinct user-page readings. -->

The detail's surviving allocation sites are 383 mapped ELF frames, 235
mapped anonymous-fault frames, 33 mapped prepared-memory frames, two
other mapped frames, 21 child page-table frames and two roots (lines
1275415–1275445). These are **after-only totals**, not site deltas.
No before site census or per-process address-space census is present.

GPU resources in lines 1275447–1275469 all belong to the kernel; the
window registry is empty, with no compositor or remnants (1275471).
Retirement reports 517 retired roots, zero abandoned/undecided/mid-retire
roots, zero lost retirement frames and no queued or parked receipts
(1275473–1275477). Those counters exclude their named failure paths;
they do not prove that all live mappings or malloc blocks are necessary. <!-- claim-lint:ok: serials/a3-memory-c48b19b9.json records includes ownership and retirement counters; the statement expressly limits their interpretation. -->

The retained earlier acceptance `20261112T040103Z-qemu-15251` at `216124b3`
has the same before owner totals, but after has 606 user pages, 11 table
pages and 15 stack pages: ten retained pages. It is a different commit,
so that pass neither establishes acceptance at this head nor establishes a race in its
memory result. <!-- claim-lint:ok: comparison is source 216124b3, retained boot 20261112T040103Z-qemu-15251 serial lines 277 and 1410903; no same-head inference is claimed. -->

## Source trace and missing discriminator

`kernel/src/memory/frame_allocator.rs:1357` aggregates allocation sites
and whether a frame has leaf references. It exports neither process/root
identity nor the virtual address referring to that frame. The anonymous
allocation site (`memory/anon_map.rs:234`) serves all private anonymous
faults; `syscall/memory_common.rs:229` serves prepared mappings. A site
name cannot distinguish a live runner allocation from a surviving
descendant, a shared reference or an unwanted retained mapping. <!-- claim-lint:ok: kernel/src/memory/frame_allocator.rs frame_census emits aggregate sites, not process or VA identities. -->

The suite sweeps descendants before the after sample (`suite.rs:1376`),
and the live unmap path has empty-table reclamation
(`memory/process_memory.rs:2183`). The existing capture has no mapping
or table census to show whether the 12 additional tables are nonempty,
which surviving ranges keep them, or whether they should have been
reclaimed. Zero pending receipts cannot answer that question. <!-- claim-lint:ok: kernel/src/memory/process_memory.rs reclaim_empty_tables and libs/libbreenix/src/suite.rs memory_failure_detail define the available boundaries. -->

The runner's libc maps each malloc block independently and drains its
free-block cache before each settled reading (`stdlib/malloc.rs:30–73`,
`suite_memory.rs:128–149`). The runner retains summary/view state until
its final panel. That makes live runner allocations a candidate, but the
capture does not name them. Changing malloc, the sweep, table reclaim,
the sampling boundary or the allowance would be speculative.

**Ledger blocker question for one instrumented boot:** at the existing
before and after boundaries, which process/root and virtual ranges own
the additional user frames and table frames? For each remaining table,
is it nonempty and what surviving VMA/leaf keeps it? For runner-owned
anonymous ranges, which malloc allocations remain live versus cached?
Retain bounded ownership snapshots before observer allocations, on every
boot, and export them only on failure, with explicit overflow reporting.
This separates necessary live state, unintended live state and failed
unmap/retirement. A deterministic reproduction must force the identified
allocation or ordering before any repair. No speculative forcing check
or runtime fix is committed here. <!-- claim-lint:ok: this paragraph proposes measurements under issue #2872, not a measured outcome. -->

## Retained timing margins

Offline extraction deduplicates wait rows by tid, label and begin time.
The excerpt records the worst row for each group with its raw line
number. End stamps include the observer's return/export bracket; they
are not GPU execution times.

| Measurement | Observed | Bound | Remaining margin |
| --- | ---: | ---: | ---: |
| Positive focus-click delivery, worst of 23 | 223.063 ms | 1000 ms | 776.937 ms (77.69%) |
| Focus-click release, worst of 23 | 14.117 ms | 1000 ms | 985.883 ms (98.59%) |
| Supervisor replacement wait | 1129.003 ms | 2000 ms | 870.997 ms (43.55%) |
| Replacement pid 707 exec to first composite | 691.556 ms | 2000 ms | 1308.444 ms (65.42%) |
| Restart-adopts kill-return to wait-return | 5.779 ms | 500 ms | 494.221 ms (98.84%) |
| Interaction restart kill-return to wait-return | 0.036 ms | 500 ms | 499.964 ms (99.99%) |
| Stack-restored kill-return to wait-return | 12.758 ms | 500 ms | 487.242 ms (97.45%) |
| Animation median | 11.943 ms | 33 ms | 21.057 ms (63.81%) |
| Animation maximum | 19.435 ms | 100 ms | 80.565 ms (80.57%) |

Frame-floor reports 2643 intervals in 4999.816166 ms, 528 Hz against
60 Hz. Its first endpoint is 0.063042 ms after measurement begin and
its last is 0.257959 ms before measurement end. This is a measured
passing result for this capture, not evidence of guaranteed margins on
other boots. <!-- claim-lint:ok: serials/a3-memory-c48b19b9.json performance contains the rate and endpoint records for this one capture. -->

All 3686 distinct **captured** wait starts have a matching end. Their
minimum recorded headroom is the supervisor wait's 43.55%. However,
the 512-row stall ring reports 49862 overwritten rows at the final
export (`kernel/src/window/stall_trace.rs:5–22`). Pairing every captured
start does not establish that every wait was captured. The full
every-click/every-wait margin audit therefore remains incomplete;
no universal 25% claim is made. <!-- claim-lint:ok: 3686 of 3686 captured starts paired; serials/a3-memory-c48b19b9.json captured_waits records the 49862 overwritten rows and coverage limit. -->

## Handoff

No check, scorer, selection, threshold or runtime input changed. No
acceptance boot, native boot, board update or merge was performed.
The next action needs the coordinator's disposition of the ownership
question, followed by causal check-first work. The existing memory
failure remains an independent blocker even though all selected cases
passed. <!-- claim-lint:ok: serials/a3-memory-c48b19b9.json records includes 81 of 81 CASE summary and independently failed memory verdict; this unit only adds report artifacts. -->
