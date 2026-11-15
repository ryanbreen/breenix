# cowfs host model

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
