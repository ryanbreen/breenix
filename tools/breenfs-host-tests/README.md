# breenfs host model and recording device

This isolated `no_std` crate imports the existing breenfs core with `#[path]` and
uses `alloc` for its independent logical oracle. The core has no dependency on
the kernel runtime; integration tests run the host formatter and checker on
temporary files. Run it with:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test --manifest-path tools/breenfs-host-tests/Cargo.toml
```

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
valid abandoned slots and flushes the cleanup before returning a mount candidate.
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
