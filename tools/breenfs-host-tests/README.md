# breenfs host model and recording device

This isolated `no_std` crate imports the existing breenfs core with `#[path]` and
uses `alloc` for its independent logical oracle. The core has no dependency on
the kernel runtime; integration tests run the host formatter and checker on
temporary files. Run it with:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test --manifest-path tools/breenfs-host-tests/Cargo.toml
```

Step 24.8r's rename is host-only under coordinator ledger lines 171, 203 and
210. The kernel does not declare breenfs in `kernel/src/fs/mod.rs`; changes to
block-cache, VFS and file-mapping files outside this core are comments only.
The renamed format magics are exercised by regenerated host images. Runtime
mount integration starts at 24.10, so this step reaches no guest suite or boot.

Unit 24.8-a-model introduced `model::seed_1_create_write_remount` as a red
sentinel for the absent writer. Unit 24.8-f supplies the file writer adapter;
that unchanged operation sequence now exercises create, sparse write, fsync
and recovery from the recording device.

`Fs::execute` consumes an `Operation` with an explicit timestamp. `Action` uses
inode-relative single-component names; VFS pathname traversal, credentials,
umask, access and set-ID policy are outside this oracle (design section 1.1).
`Create` means exclusive creation (O_CREAT|O_EXCL), not creat(2) truncation.
Creation starts with uid/gid 0; `setattr` supplies owner changes. `lookup` and
`read` on the model are observations and do not update atime. Symlinks are stored
as bytes and are not followed by these inode-relative operations. No file
handles are modeled yet, so the last unlink removes the inode immediately.

The oracle follows POSIX.1-2017's create/write/truncate/link/unlink/rename/mkdir/
rmdir/symlink/chmod/chown/utimens semantics at that boundary. It selects EPERM
for directory unlink, ENOTEMPTY for nonempty replacement, and EINVAL for a
rename into its own subtree. The representation uses the inode numbers and
limits in `docs/fs/breenfs-design.md`: root 2, new inodes from 16, monotonic ids,
and bounded names, files, symlinks and xattrs. Xattrs are a breenfs extension,
not a POSIX interface: invalid names/values return EINVAL, missing removal
returns ENOENT, and set replaces or inserts.

`fsync` acknowledges the shared transaction under design section 2.5, including
namespace changes. `durable_state` retains that acknowledged state. `remount`
is a clean unmount/mount that commits pending changes, not a crash or a rollback
to the last fsync. `tests/commit_recover.rs` checks commit publication faults
and recovery; the exhaustive crash-image corpus belongs to step 24.9.

`Stream::new(seed)` begins with create, sparse write, fsync and remount. Its
subsequent xorshift64* choices are reproducible without dependencies, host
entropy or a wall clock. A private model tracks candidate inode numbers and
names; failed operations stay in the stream for errno comparison. Consumers
bound the iterator with `take` and replay the identical operations on the model
and writer, comparing replies and then `compare(model, fs)` after each call.

`Fs::state` returns a canonical logical view keyed by inode number. The writer
adapter must expose directory entries/parents, nlink, modes, owners, timestamps,
symlink targets, xattrs and file bytes. The byte map omits values equal to 0;
size distinguishes a hole from EOF. Physical extents, allocated blocks and
directory cookies are outside this POSIX state. `compare` returns inode diffs
with expected and actual records, including missing and unexpected inodes;
errors from either observer propagate.

The model is shared across architectures. ARM still lacks the transaction
writer adapter and writable kernel breenfs integration, as does x86 in this unit;
ARM64 QEMU, Parallels and VMware acceptance remains for later steps.

`recording::RecordingDevice` implements the host `block::BlockDevice` fixture
(the same synchronous surface as `tools/block-host-tests`) with 512-byte sectors.
Its initial image is durable. Reads see completed writes; `durable_image` changes
at flush, and `events` retains sector bytes and flush boundaries for replay.
Calls serialize through a mutex, so a returned write has completed and a flush
cannot pass an outstanding write in this fixture.

The device enforces the writer rules in `docs/fs/breenfs-format.md` section 3:

- W1 protects the two highest durable generations. The device walks the encoded
  SPACE tree, ROOT trees, DEAD nodes and extents using the kernel format and
  B-tree readers. It protects the four blocks of a node and the full physical
  allocation of a data REF, including slices retained by snapshots or DEAD nodes.
- W2 requires a first-flush fence before ring writes. A valid superblock's nodes
  and data must be readable from the flushed image, with matching pointers and
  data CRCs. A pending superblock's reachable blocks cannot be rewritten before
  its following flush.
- W3 publishes a pending superblock and advances the protected generations only
  when the following flush completes. A data-only flush leaves that set alone.

A writer-rule breach panics with its W number and latches the run as failed;
later I/O panics too. Logs and the durable image remain inspectable. Short
buffers and sector bounds errors return `BlockError`. Constructor errors reject
unreadable initial trees or disagreeing UUIDs. Only the two newest readable
generations are retained, with duplicate ring copies counted once.

The four recording checks run the real `breenfs-mkfs` binary, built as a Cargo
artifact dependency, then hand-script sector writes and flushes. They exercise
old-generation node protection, ROOT nodes and data, the first-flush fence,
durability publication, and legal reuse after the older commit ages out. These
scripted generations exercise W1-W3; the independent checker owns acceptance
of the image invariants. The same host recorder serves x86-64 and ARM64; this
unit adds no guest runtime acceptance.

`space.rs` is imported from the kernel path without declaring breenfs in the kernel.
It edits REF, FREE, PENDING and DEAD items through the generic B-tree and consumes
caller-supplied data CRCs. A private transaction store must roll back a failed
store operation. `add_free` accepts unowned, nonreserved ranges; allocation takes
FREE before converting eligible PENDING ranges, and coalesces adjacent FREEs.
`release` takes the releasing commit generation and whether an FS node still
owns child/data references. Such a zero-ref node becomes DEAD without changing
its children; the later transaction/reaper units own those decrements.
Space nodes remain uniquely referenced. This unit does not supply the writer,
reservation policy, commit engine, reaper or guest mount on either architecture.


`txn.rs` stages a transaction's FS nodes in a private overlay and uses the B-tree's
`prepare_edit` hook before descending or rebalancing siblings. Earlier-generation
nodes get fresh addresses; generation-T nodes keep their addresses on later edits.
A shared FS copy increments its internal children, or the physical REF named by
each EXTENT item in a leaf, before descending. A uniquely referenced copy transfers
its outgoing references. Structural merges and root collapse transfer references
and release detached nodes through `space.rs`.

Space edits are deferred. A separate scratch view runs `space::allocate` to reserve
addresses with its FREE/PENDING reuse policy. `settle` applies those allocations,
refcount changes, releases and ROOT records to the COW space tree, including work
queued by that tree's own copies and splits. The scratch split addresses stay in
the reservation view; physical node writes use addresses reserved by `space.rs`.
An error poisons the transaction; discard it and its unpublished roots. The backing
store is borrowed read-only and retains the earlier node bytes.

`allocate_data` returns one caller-owned reference; inserting its EXTENT transfers
that reference. Another item naming the same physical REF needs `increment` first.
Replacing an EXTENT that names a different REF, or deleting one, releases the old
reference. `update_root` transfers the reference established by COW or `create_tree`
into the ROOT record. The later operations layer owns namespace/item semantics.

Run the four transaction checks with `cargo test --test txn`. They cover repeated
edits, shared leaf extents, the space-tree fixpoint, and seeded insert/delete streams
against BTreeMap while the earlier root retains its values. The transaction core
is architecture-independent and remains undeclared in `fs/mod.rs`. ARM and x86
still lack commit/recovery/ops, the writer adapter, reservation policy, the reaper
and writable kernel mount integration. ARM64 QEMU, Parallels and VMware runtime
acceptance remains in the later steps.

`commit.rs` now publishes a settled transaction directly over `&dyn BlockDevice`.
It fills private node headers and pointers in dependency order, writes device-sized
sectors, flushes nodes/data, writes slot G mod 4 in both rings, then flushes the
publication. Fsync completion receipts become successful after the second flush.
An I/O failure completes the pending receipts with EIO and latches the committer
read-only. Prepublication errors issue no superblock; publication errors stop
without rollback writes, per the corrected design section 2.5.

`recover.rs` validates the eight slots, refuses disagreeing UUIDs, and selects the
highest generation with matching SPACE and ROOT pointers. Writable recovery clears
valid abandoned slots, restores the selected generation's missing or invalid ring
copy, and flushes those writes before orphan reclaim or returning a mount candidate.
A cleanup error refuses that writable recovery. This is a root validation pass;
FS descendant checking on reads and the full scrub remain separate layers.

Run the seven commit/recovery checks with `cargo test --test commit_recover`.
They use the recording device, including failure wrappers, two publication
outcomes, a retained private DEAD node, internal FS nodes, and a logical tree
comparison against the POSIX model. The model's absent-writer sentinel remains
with the later operations/adapter units. ARM and x86 still lack that adapter,
reservation policy, DEAD/orphan reaping and writable kernel mount integration.
The kernel module is still undeclared; ARM64 QEMU, Parallels and VMware runtime
acceptance and the Hold-group adapter belong to later steps.


Unit 24.8-f adds `ops.rs` for regular-file creation, sparse reads/writes,
truncate, setattr and xattr insertion/replacement/removal. New inode numbers
come from ROOT.next_inode. Creation publishes paired directory hash/index items.
Writes allocate private 4 KiB data blocks with CRCs in REF; holes have no items.
Truncate clears the portion after EOF in a mapped partial tail and releases
removed EXTENT references through txn.rs into PENDING. Splitting an existing larger
extent retains a reference per surviving slice. Xattr collision buckets use the
superblock SipHash key and the design's name/value limits.
<!-- claim-lint:ok: File representation requirements in docs/fs/breenfs-design.md section 2.3; kernel/src/fs/breenfs/ops.rs implements truncate. -->

`WriterAdapter::new` takes a device, recovers root 1 and retains an owned
read-only store in its transaction. Its `Fs` implementation calls the kernel
file operations; `state` decodes tree items and reads file bytes without consulting
the model. Fsync publishes the transaction; clean remount commits dirty work,
drops the overlay and recovers from the device. Storage failure prevents further
operations on that adapter. Preflight ENOSPC reservation remains step 24.12.

Run this unit's three checks with `cargo test --test ops` and the seed with
`cargo test --lib model::seed_1_create_write_remount`. The checks compare logical
state after operations, inspect PENDING on disk, read holes directly, and invoke
the actual `breenfs-check --full` binary after their fsync commits, requiring exit 0.
The recording device checks W1-W3 during publication. Unit f does not move mkfs
population onto the writer; that follows the namespace half of operations.

ARM and x86 still lack namespace mutation beyond regular-file creation, orphan
and DEAD reaping, reservation policy, Hold-group/page-cache integration and the
writable VFS mount/runtime acceptance. The architecture-neutral core remains
undeclared in kernel fs/mod.rs. ARM64 QEMU, Parallels and VMware runtime coverage
belongs to later steps; this unit has no boot.

Unit 24.8-g adds mkdir/rmdir, hard links/unlink, replacement rename and inline
symlinks to `ops.rs` and the host writer adapter. Directory entry counts, parent
links and monotonic index cookies accompany the paired hash/index records.
Rename validates directory replacement and ancestor chains before changing names.
Last unlink records an ORPHAN with nlink 0; the logical adapter hides that inode.
Commit removes its items and releases extents through the transaction engine.
Writable recovery resumes committed ORPHAN work using the same commit path;
read-only recovery leaves the image alone. Open-handle lifetime remains a later
VFS integration task, since this adapter has no open handles.

`model::seed_corpus_matches_model` compares replies and logical inode state for
seeds 1 through 8, 256 operations per seed, after each operation and remount. It
requires the independent full checker to accept newly committed generations.
The two namespace checks in `tests/ops.rs` cover replacement and refusal paths,
symlink bounds, committed-orphan recovery and a real SipHash-2-4 collision under
key `00000000000000000000000000000000`. Cookie and collision-bucket assertions
inspect encoded items rather than the model's name map. The recording device
continues to enforce W1-W3. The checker fixture uses a per-process invocation
counter so parallel checks with identical tags cannot share a temporary image.

ARM and x86 still lack writable breenfs VFS integration, open-handle lifetime,
DEAD reaping, reservation policy, Hold-group/dirty-page integration and runtime
acceptance. These modules remain undeclared in kernel fs/mod.rs; this unit
cross-builds the actual no_std modules for both architectures and runs no boot.
Moving mkfs population onto the writer remains the next unit.

Unit 24.8-h moves the formatter onto `ops.rs` and `commit.rs`. The transaction
bootstrap starts a private generation-1 SPACE leaf at block 20 with its own REF
and a FREE range beginning at block 24. FS creation, namespace population, data
allocation and the deferred SPACE fixpoint remain in one transaction. The
committer uses an unpublished generation-0 sentinel to publish generation 1,
clears the six unused ring slots, writes the two active slots and flushes.
`--extents` reports the file device's submitted block ranges, including both rings.
Contiguous file writes allocate runs of at most 256 blocks through the shared
operations layer, preserving the maximum-extent/tail violation fixture.
Source traversal is sorted; metadata restoration follows children and data writes.
A formatter-only generation-1 operation restores source ctime and btime without
changing setattr semantics for later transactions. Population keeps the existing
root ownership, source modes and timestamp behavior, including `--time` overrides.

ARM and x86 share this formatter and core. Writable VFS integration, open-handle
lifetime, DEAD reaping, reservation policy, Hold-group/dirty-page integration and
ARM64 QEMU/Parallels/VMware runtime acceptance remain later steps. This host unit
runs no boot and adds no architecture-specific writer implementation.

## Flush-boundary subset crash images (24.9-a)

`crash::subset_images(initial, seed, operations, checker)` replays a bounded
`Stream` on the POSIX model and `WriterAdapter` over `RecordingDevice`. It compares
operation replies and logical state, records the model at each published commit,
and ends with a clean remount to publish pending work. The recorder continues to
check W1-W3 on the source writes.

At a recorded flush, the previous durable image is combined with selections of
the intervening 512-byte writes, preserving issue order. The selection policy is
breenfs-design.md section 5: exhaustive subsets through 12 writes; above that,
prefixes, single omissions and 4,096 seeded random subsets. Duplicate selections
still go through validation. The implementation retains the fsync acknowledgement
that preceded the boundary and advances the upper bound when the superblock's
first sector is issued.

Recovery uses a separate plain in-memory sector device with sparse storage.
Writable `WriterAdapter::new` can clear abandoned slots or reclaim orphans there.
The injected checker receives the recovered image, including cleanup writes, and
the recovered logical state must match a model snapshot within the inclusive
acknowledged-to-issued commit interval. This device carries no recording oracle.

Run the focused check with:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test \
  --manifest-path tools/breenfs-host-tests/Cargo.toml --test crash_images \
  subset_images_recover_to_an_acknowledged_commit -- --exact --nocapture
```

The host crate's dev/test profile optimizes node decoding and checksumming,
including its artifact binaries, with debug assertions and overflow checks enabled.
The fixed corpus uses seeds 1 and 2, eight stream operations per seed, and the
final remount. Its checker closure materializes a fresh sparse image and invokes
the Cargo artifact `breenfs-check --full`, requiring exit 0 for each image. Sparse
representation reduces copying and file traffic; it does not reduce the selected
images or the recovery, checker and model validations. The same check covers the
12/13-write selection boundary.

This unit is host acceptance only. Kernel `fs/mod.rs` still does not declare
breenfs. ARM and x86 still lack the read/write VFS mount integration, open-handle
lifetime, DEAD reaping, reservation and Hold-group/dirty-page integration; ARM64
QEMU, Parallels and VMware runtime acceptance remains later work. Bit flips,
minimization, torture and host milestone records belong to the later 24.9 units.

Coordinator ledger line 217 settles
[#3020](https://github.com/ryanbreen/breenix/issues/3020): writable recovery must
restore the selected generation's missing or invalid ring copy and flush before
its first commit. Read-only recovery leaves the image alone. The focused
`recover::writable_recovery_restores_the_missing_ring_copy` check uses a plain
memory device with separate visible and durable bytes. It requires the ring-B
slot writes followed by a flush, without a requested commit, and read-only
recovery without writes. Missing and invalid copies exercise the same contract.
The subset corpus and the checker's exit-0 requirement remain unchanged.

## Torn crash images (24.9-b)

`crash::torn_images(initial, seed, operations, tail, checker)` uses the same
stream, flush boundaries, sector-write masks and model commit interval as the
subset corpus. For each mask it starts from the preceding durable image and
applies the selected sector writes in issue order. Each 4 KiB block written
since that flush then retains a seeded prefix of one through seven sectors
from that selected image. Its remaining sectors come from the preceding durable
image (`TornTail::Old`) or a seeded xorshift64* byte stream (`TornTail::Garbage`).
The seed includes the stream seed, boundary, mask index and block address;
both tail kinds use identical prefix lengths. Unselected sectors inside a
retained prefix stay old. No block outside that window's writes is modified.

The fixed check runs both tail kinds over seeds 1 and 2, eight stream operations
and the final remount, within a ten-minute bound. Each image receives writable
recovery, a separate invocation of the independent `breenfs-check --full` binary
requiring exit 0, and logical comparison with a model commit no earlier than
the last acknowledged fsync and no later than the last issued superblock.
Failures identify the seed, operation, boundary, mask index and tail kind.

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test \
  --manifest-path tools/breenfs-host-tests/Cargo.toml --test crash_images \
  torn_images_recover_to_an_acknowledged_commit -- --exact --nocapture
```

The check was committed against an `Unimplemented` stub before the replay
extension. The architecture-neutral core remains undeclared in kernel
`fs/mod.rs`. ARM still lacks VFS read/write mounts, open-handle lifetime,
DEAD reaping, reservation, Hold-group/dirty-page integration and ARM64 QEMU,
Parallels and VMware runtime acceptance. This unit runs no boot.

The garbage-tail corpus found seed 1, boundary 1, mask 0 leaving nonzero invalid
slots A2/B2 after recovery. `crash::tests::seed_1_garbage_tail_in_unpublished_ring_slots`
retains that minimized case. Writable recovery now clears nonzero invalid slots
after validating the selected generation, restores its companion directly, and
flushes before exposing the mount. Read-only recovery retains the original bytes.
The checker and the corpus acceptance criteria are unchanged.

## Committed-image corruption (24.9-c)

`corrupt::bit_flips` takes a populated formatter image and the durable image
after successful fsync calls in the fixed corpus (seeds 1 and 2, eight stream
operations). It enumerates valid slots in both rings and walks the selected
generation's SPACE tree, ROOT trees and DEAD nodes. Node targets include their
four blocks; data targets include the physical allocations named by REFs and
EXTENTs. Historical trees are outside the selected generation's reachable set.
A seeded bit offset selects one flip in a separate image for each block.

`tests/corruption.rs::bit_flips_error_never_panic_and_checker_names_the_block`
invokes the independent `breenfs-check --full` on the corrupted image before
recovery can alter it. Slot and node flips require nonzero exit and a diagnostic
naming the block or covering node address; data flips require C1 naming the
block. Separate devices exercise read-only and writable recovery. Read-only
recovery must preserve the image. Adapter construction, the full logical state
walk and reads of the pristine image's regular-file inode/length list run under
`catch_unwind`; errors are permitted and a panic fails the check. The reads
include holes, partial tails and EOF, even if the state walk returns an error.
Each flipped node also goes directly through the production B-tree search path,
which must return EIO even if recovery selected an older generation or the file
reads avoid that node. The pointer is read from raw fields without a preliminary
CRC decode. Checker diagnostics must name a block or node explicitly on an error
line, or name the node CRC that forced abandonment of a newer generation.
Data EIO enforcement remains step 24.13.

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test \
  --manifest-path tools/breenfs-host-tests/Cargo.toml --test corruption \
  bit_flips_error_never_panic_and_checker_names_the_block -- --exact --nocapture
```

The check first failed against the corpus stub in commit `45867bd5`.
With replay implemented, the existing decoders and checker satisfy it, so no
kernel decoder or checker change is needed (U29). The core remains undeclared
in kernel `fs/mod.rs`. ARM still lacks VFS read/write mounts, open-handle
lifetime, DEAD reaping, reservation, Hold-group/dirty-page integration and
ARM64 QEMU, Parallels and VMware runtime acceptance. This unit runs no boot.

## Seed minimization and regression rows (24.9-d)

`minimize::Case` names a failure by its seed, its stream length, the kept
stream indices and a fault. A `Fault::Crash` holds the flush boundary of the
case's run (its final clean remount included), that window's 512-byte write
count, the ascending indices of the selected writes, and the torn tail and
tear seed if any. A `Fault::Flip` holds a `corrupt::Flip`. Kept operations are
the seeded stream's own, so dropping one leaves later operations unchanged.

`minimize::minimize(case, fails)` is `no_std` library code with a caller-supplied
predicate, so a host binary can drive it with a test's acceptance. It requires a
well-formed failing case (otherwise EINVAL) and shrinks it in three
deterministic steps: the shortest failing stream prefix; single operations
dropped, latest first, until no single drop still fails; then single selected
crash writes dropped the same way. The predicate decides what counts as the same
failure and returns false for a candidate it cannot apply. `regression_row`
prints the result as a Rust expression for `tests/crash_regressions.rs`.

`crash::case_at(initial, seed, operations, boundary, image, tail)` turns the
coordinates a subset or torn failure reports into a case.
`crash::replay_case` and `corrupt::replay_flip` replay one row through the same
code as the corpora: the recorded source run with W1-W3, writable recovery, the
injected checker and the model commit bounds for crash rows; the committed image
after the kept fsync and the corpus observer for flip rows. A row whose boundary,
write count, fsync or block kind is absent from its run is `Inapplicable`.

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test \
  --manifest-path tools/breenfs-host-tests/Cargo.toml --test minimize --test crash_regressions
```

`tests/minimize.rs::minimizer_reduces_a_failing_stream` injects a predicate that
fails when a successful rename precedes a successful fsync and the crash image
keeps write 1. A seeded 256-operation stream reduces to the rename, the fsync
and the creating operations they need, with write 1 alone selected. Stream
operations name inodes by number, so a create that allocates an inode number
the rename or fsync names is required as well. For seed 1 that is 8 of 111
prefix operations: six creates, links, directories or symlinks, then the
rename and the fsync. Removing any one kept operation passes, and a second run
emits the same row.

`tests/crash_regressions.rs::every_minimized_case_replays` holds one row. The
24.9-b garbage-tail failure (seed 1, 8 operations, boundary 1, image 0) was
located with `case_at` and minimized against the recovery of commit 021da596,
before 24.9-b's repair, where the checker exits 4 naming slots A2 and B2. It
reduces to the first create and the final remount at the publication boundary,
with no selected writes. At this head the row passes the crash-image acceptance:
the full checker exits 0 and the recovered tree matches a commit in bounds. The
24.9-c corpus had no failing flip, so the table has no flip row; flip rows use
`tests/corruption.rs`'s observer and its populated formatter fixture.

This unit is host acceptance only. The core remains undeclared in kernel
`fs/mod.rs`. ARM still lacks VFS read/write mounts, open-handle lifetime,
DEAD reaping, reservation, Hold-group/dirty-page integration and ARM64 QEMU,
Parallels and VMware runtime acceptance. This unit runs no boot.

## Seed-bounded torture driver (E24 coordinator ledger line 267)

Build and run the host binary with an independent checker:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo build \
  --manifest-path tools/breenfs/Cargo.toml --bin breenfs-check
CARGO_BUILD_JOBS=4 taskpolicy -b cargo build \
  --manifest-path tools/breenfs-host-tests/Cargo.toml --bin breenfs-torture
taskpolicy -b tools/breenfs-host-tests/target/debug/breenfs-torture \
  --seeds 2 --seed-start 1 \
  --checker tools/breenfs/target/debug/breenfs-check --out /tmp/breenfs-torture
```

The command accepts only `--seeds N [--seed-start S] --checker PATH --out DIR`.
The seed count must be a positive integer; the default first seed is 1.
The inclusive range must fit in u64. A seed runs `32 + (seed % 4) * 8` stream operations plus the final remount through
`crash::subset_images`, both `crash::torn_images` tails, and
`corrupt::bit_flips`, using the existing full selection policy and acceptance.
The high bit of the emitted stream seed tags the torture workload, preserving
that choice when a regression row is minimized; summaries use the consecutive
input seed range. Ordinary untagged streams retain their previous operations.
After the historical eight-operation prefix, the torture stream adds 4 KiB xattrs to
split the filesystem tree, multi-block data writes, overwrites, truncations and
repeated commits to wrap the superblock slots. The source check in
`tests/torture.rs::torture_stream_reuses_slots_and_splits_trees` requires seeds
1 and 2 to reach generation 9 and tree height 2.
Fixtures are built directly through the shared `Files::format` and commit
entry points, with fixed metadata and a populated tree containing multi-block
data, a partial tail, an empty file and a symlink. No formatter executable is
required by the driver. The checker is invoked as `PATH --full IMAGE`.

The driver stops after the requested count of complete seeds. Seeds advance
consecutively without wrapping. A partial seed cannot satisfy the count.
Exit 0 requires the requested seed range to finish without failure.

`DIR/summary.json` is replaced atomically at startup and after a seed. Its fields
are `started` (Unix seconds), `seeds_requested`, `elapsed` (monotonic seconds),
`first_seed`, `last_seed` (inclusive; null before a seed), `images_checked`
(checker invocations, including minimization), `deepest_generation` and
`tree_height` (greatest selected generation and reachable height among recovered
crash images; a leaf has height 1), and `failures` (failed seeds).
On failure the driver publishes the failed seed and failure count before
minimizing. `DIR/failure-<seed>.txt` holds the diagnostic and the minimized Rust
row from `minimize::Case::regression_row`, replayed against the same failure
kind. It then exits 1. A failed run retains its last sparse checker input;
a successful run removes that scratch image. Setup or output I/O errors also
exit 1.

`tests/torture.rs::one_seed_writes_a_summary` runs the built binary with
`--seeds 1` and Cargo's artifact checker. It requires exit 0, checked images,
the exact one-seed range and a failure count of 0. The original duration-based
check-first commit was `75ba4f78`; seed-range parsing is checked by `61ad8d8e`.
Coordinator ledger line 267 replaces timed acceptance with seeds 1-8, run after
the acceptance boot as four processes of two seeds each at nice 15.
The PR records the range, image count and failures.

This host driver adds no architecture-specific path. ARM still lacks VFS
read/write mounts, open-handle lifetime, DEAD reaping, reservation,
Hold-group/dirty-page integration and ARM64 QEMU, Parallels and VMware runtime
acceptance. This unit runs no boot.

## Verified node reads (24.10-a)

`node_cache::CachedNodeStore` reads through `BufferCache::read_verified` using
one four-block node per cache entry. Its private device identity separates node
verification from raw-sector readers and other mounts; dropping the store
retires its clean entries. Admission checks the node CRC, requested address,
nonzero generation bounded by the durable superblock, and filesystem UUID.
The B-tree retains its exact parent-pointer checks. Allocation and writes refuse.

`tests/node_cache.rs` uses a populated formatter image. It checks clean reads
and cache hits on 512-byte and 4 KiB devices, verification failures followed by
device retries, and successful reads after repair. Header mutations carry a
recomputed CRC so the address, generation and UUID checks are exercised directly.

The core stays undeclared in kernel `fs/mod.rs`. ARM and x86 still lack the
read-only VFS mount integration; ARM64 QEMU, Parallels and VMware runtime
acceptance remains later work. This unit runs no boot.
