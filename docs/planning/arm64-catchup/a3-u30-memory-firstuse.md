# A.3 U30: ownership measurement before a memory repair

## Problem and code decision

The retained runtime is `c48b19b9`, capture
`20261112T051028Z-qemu-76740`. Its MEMORY result is 55 retained pages
against 48: +44 user, +12 table, -1 stack. The excerpt and source records
are in [the earlier audit](a3-memory-blocker.md) and
[its serial excerpt](serials/a3-memory-c48b19b9.json). Coordinator ledger
line 282 supersedes that audit's stop condition and requests either a
code-supported first-use repair or bounded ownership measurements.

The expected surviving user process is PID 1, `suite-window-manager`.
The capture's boot records launch that binary as PID 1; its runner stays
in `Suite::run` through both samples. This launch is standalone, not a
persistent desktop session. `suite_window_manager.rs` uses
`.defer_display()`, not the GUI/subreaper role. `suite.rs::sweep_for_role`
therefore selects `sweep`: PID 1 sends SIGKILL to the other processes and
reaps children, after each case and again before the after sample.
`wm_suite/desktop.rs::Child::stop` also kills and reaps owned children.
The compositor and clipboard/client processes are suite descendants,
not services intended to survive its final sweep. Kernel worker threads
continue running, but do not run compositor/client user address spaces.
The failure's empty window registry agrees with this lifecycle; it is
not a process inventory and cannot exclude leaked mappings.
<!-- claim-lint:ok: libs/libbreenix/src/suite.rs sweep, sweep_for_role and Suite::run; userspace/programs/src/suite_window_manager.rs main; retained serial boot records before line 277 and registry at line 1275471. -->

Pipeline preparation has **zero direct user-page or user-page-table
allocations per pipeline**. `bgl::Context::for_pipelines` sends the mask
through `gpu::context_create_for`; its command and clip vectors start
empty. In the kernel, `gpu::prepare_window_renderer_for` and
`create_context_with_requirements` prepare contexts and internal GPU
resources. `gpu::virgl::encode::bind_programs` allocates two object
handles, serializes the static vertex/fragment programs into kernel
vectors, and saves the handles in the context's fixed catalogue array.
`encode::shader` uses a kernel `Vec<u32>`. These operations do not call
user mmap, install user leaves, or create a user's page tables.
<!-- claim-lint:ok: libs/bgl/src/lib.rs Context::for_pipelines; kernel/src/gpu/mod.rs create_context_with_requirements; kernel/src/gpu/virgl/encode.rs bind_programs and shader. -->

Drawing can grow a client's command vectors and font caches through
malloc, which maps anonymous user memory. That is data-dependent client
work, not a fixed user-page cost of preparing a pipeline. Those clients
are reaped before the after sample. No count of pipelines in this code
accounts for +44 user and +12 table pages retained by the surviving
runner. The first-use hypothesis is therefore not confirmed, and no
background compositor warmer is added. The exact remaining process/root
inventory and the ownership of those extra pages require measurement.
<!-- claim-lint:ok: libs/bgl/src/lib.rs Context words/clips and draw; libs/libbreenix-libc/src/stdlib/malloc.rs malloc/free; libs/libbreenix/src/suite.rs final sweep precedes memory_sample. This is a limit on attribution, not a leak diagnosis. -->

## Measurement change

`/proc/breenix/memory-owners` captures up to 2048 rows into a static kernel
scratch array under the process-manager lock. The production walker is
shared by ARM64 and x86. Rows identify process PID/name/root, VMAs with
protection/mapping flags, user leaf VA/physical frame/size/flags, and owned
tables with level, covered VA range, present-entry count and nonzero-entry
count. Table rows give the count and first overlapping VMA; the VMA rows
retain the remaining overlaps. Physical addresses permit shared-leaf
and thread/root deduplication. Pending old roots and detached table leases
are named separately; a detached table has level 0 because it has no
reachable parent from which to derive a level or VA. A full buffer sets
`overflow=1`; incomplete evidence must not be treated as complete ownership.
<!-- claim-lint:ok: kernel/src/memory/ownership_snapshot.rs walk and Snapshot::push; kernel/src/memory/process_memory.rs snapshot_ownership; kernel/src/fs/procfs/memory_owners.rs generate. -->

The endpoint requires the existing root-only Trace privilege. Concurrent
capture returns EAGAIN. It constructs text after capture and releases the
process-manager lock before formatting. The open file description retains
that immutable text in kernel memory. Each memory reading opens it using
the allocation-free syscall wrapper before reading meminfo or frame
census strings; the Sample retains just the close-on-exec fd. Its Drop
closes intermediate readings and the final retained fd. Both final
snapshots are exported as `SUITE_MEMORY_OWNERS <suite> before|after ...`
only when reclamation fails. The existing counts, settle rules, sampling
position and 48-page allowance are unchanged.
<!-- claim-lint:ok: kernel/src/fs/procfs/mod.rs MemoryOwners; libs/libbreenix/src/fs.rs CPath/open; libs/libbreenix/src/suite_memory.rs Ownership/read/settled; libs/libbreenix/src/suite.rs failed-verdict export. -->

The runner drains its free-block malloc cache immediately before each
reading, so its cached malloc blocks have been unmapped at capture.
Remaining runner anonymous VMAs include live malloc blocks and any direct
mmap allocations. The snapshots identify those mappings without guessing
that every anonymous mapping is malloc. They capture ownership before the
current reading's allocating observers, not a globally atomic census of
asynchronous kernel/device work. Snapshot open failures are exported as
unavailable evidence rather than silently dropping a boundary.
<!-- claim-lint:ok: libs/libbreenix/src/suite_memory.rs release_allocation_cache precedes read; libs/libbreenix-libc/src/stdlib/malloc.rs __breenix_malloc_release_cached unmaps drained blocks; Ownership::detail reports capture/read errors. -->

## Checks and next action

The check was committed first as `6617f4dc`. On the unchanged source
(runtime `c48b19b9`, documentation head `92fcc725`) both checks failed:
no pre-observer ownership capture and no bounded walker. With the change,
2/2 host checks pass. The production walker fixture forces an empty
retained leaf table, verifies its level/range and zero entry counts,
checks leaf PID/root/VA/physical identity, forces overflow with capacity 2,
and counts zero allocations while capturing. The sampling/export check
requires capture before allocating reads and export of both boundaries
inside the failure branch. A fixture compilation initially caught an
unread name field under `-Dwarnings`; its assertion now reads the field.
<!-- claim-lint:ok: tests/memory_ownership_snapshot_test.py and tests/suite_collection_test.py run_rust; unit host runs recorded exit 1 before implementation and exit 0 with implementation. -->

Validation notes (local logs retained under `/tmp/a3-u30-*`):

- ARM64 production release kernel: exit 0, 1m33s. No source warning;
  Cargo repeats the pinned-toolchain core future-incompatibility advisory
  already present in the preceding capture's launch log.
- x86 release kernel/launcher (`testing,external_test_bins`): exit 0,
  2m04s, no warnings.
- ARM64 scoped release suite build: exit 0, 19.03s, no warnings.
- x86 scoped release suite build: exit 0, 18.70s, no warnings.
- Initial ARM64 suite invocation refused an uncanonicalized sysroot path
  before compilation; retry uses the canonical path.
- `scripts/claim-lint.py --files tests/memory_ownership_snapshot_test.py`
  -> exit 0 before the check commit.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u30-check-msg` -> exit 0.
- `scripts/claim-lint.py --commit-msg /tmp/a3-u30-change-msg` -> exit 0.
- `scripts/claim-lint.py` -> exit 0 before the check commit and after source
  implementation (changed hunks). A whole-file invocation including
  process_memory.rs returned exit 1 for 52 pre-existing prose findings;
  that file's unchanged prose is outside this unit.
- `scripts/claim-lint.py --files docs/planning/arm64-catchup/a3-u30-memory-firstuse.md` -> exit 0.
- The first whole-file lint of ownership_snapshot.rs, memory_owners.rs and
  suite_memory.rs returned exit 1 for a new formatter comment; it was
  rewritten to describe the lock release directly.
- The first report lint returned exit 1 for two attribution phrases;
  the report now describes the limited capture evidence directly.
- The staged lint then returned exit 1 for this note quoting a flagged
  term; the shell continued to commit. This wording correction removes
  that term, and subsequent commit checks stop on failure.

No boot was run in this unit. The next ARM boot is the coordinator-line-282
**ownership measurement**, under the committed gate and headless launcher.
Its question is which PID/root/VA leaves and nonempty or VMA-retained
tables own the extra pages at the unchanged MEMORY boundaries. Compare
physical frames as well as VA ranges; if a snapshot overflows or is
unavailable, attribution is incomplete. This is not yet a reclamation fix
or acceptance evidence. Issue #2872 remains open. No R240 boot, PR creation,
board update or merge is performed here.
<!-- claim-lint:ok: coordinator ledger line 282 authorizes the ownership question; this unit records host/source work and does not claim boot acceptance. -->
