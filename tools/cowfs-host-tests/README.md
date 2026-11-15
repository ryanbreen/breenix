# cowfs host model and recording device

This isolated `no_std` crate imports the existing cowfs core with `#[path]` and
uses `alloc` for its independent logical oracle. It has no dependency on the
kernel runtime or on a host filesystem. Run it with:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b cargo test --manifest-path tools/cowfs-host-tests/Cargo.toml
```

Unit 24.8-a-model deliberately leaves `model::seed_1_create_write_remount` red:
`WriterAdapter` returns `Error::Unimplemented`. That sentinel is distinct from
`Error::Errno`; a later writer unit replaces the adapter and satisfies the same
check. The model checks exercise errno, sparse bytes, hard links, metadata,
namespace changes and whole-state differences.

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
limits in `docs/fs/cowfs-design.md`: root 2, new inodes from 16, monotonic ids,
and bounded names, files, symlinks and xattrs. Xattrs are a cowfs extension,
not a POSIX interface: invalid names/values return EINVAL, missing removal
returns ENOENT, and set replaces or inserts.

`fsync` acknowledges the shared transaction under design section 2.5, including
namespace changes. `durable_state` retains that acknowledged state. `remount`
is a clean unmount/mount that commits pending changes, not a crash or a rollback
to the last fsync. Crash recovery belongs to a later unit.

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
writer adapter and writable kernel cowfs integration, as does x86 in this unit;
ARM64 QEMU, Parallels and VMware acceptance remains for later steps.

`recording::RecordingDevice` implements the host `block::BlockDevice` fixture
(the same synchronous surface as `tools/block-host-tests`) with 512-byte sectors.
Its initial image is durable. Reads see completed writes; `durable_image` changes
at flush, and `events` retains sector bytes and flush boundaries for replay.
Calls serialize through a mutex, so a returned write has completed and a flush
cannot pass an outstanding write in this fixture.

The device enforces the writer rules in `docs/fs/cowfs-format.md` section 3:

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

The four recording checks run the real `cowfs-mkfs` binary, built as a Cargo
artifact dependency, then hand-script sector writes and flushes. They exercise
old-generation node protection, ROOT nodes and data, the first-flush fence,
durability publication, and legal reuse after the older commit ages out. These
scripted generations exercise W1-W3; the independent checker owns acceptance
of the image invariants. The same host recorder serves x86-64 and ARM64; this
unit adds no guest runtime acceptance.

`space.rs` is imported from the kernel path without declaring cowfs in the kernel.
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
