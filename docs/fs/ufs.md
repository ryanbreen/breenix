# User filesystem bridge ABI (NS.1)

This is the interface used by `memfsd_test` and implemented by the kernel bridge
in NS.1's later units. It is Breenix's own protocol, designed from the plan and
ordinary filesystem operations; it does not import a third-party filesystem ABI.
`libs/ufs-abi` supplies framing and payload primitives shared by both endpoints.

## Mount and endpoint lifetime

A root daemon opens `/dev/ufs` with read/write access, then calls
`mount(source, directory, "ufs", 0, &fd)`. The data argument points to a
native-endian `u32` containing that daemon's endpoint descriptor. The kernel
copies it during the syscall and binds that endpoint to one mount. `source` is
a descriptive label (`memfs` for the test daemon), not a path to open. The mount
root's node id is `payload::ROOT` (1). Mount and umount require effective uid 0;
the credential check precedes endpoint and path validation. An endpoint can be
bound only once. Nonzero mount flags are refused in this initial interface.

An endpoint read obtains one complete request. A buffer too small for the next
message fails without consuming it. An endpoint write submits one complete
reply; a short write is not a successful reply. Reads block when no work is
queued; interrupted reads may be retried. Unmount detaches the namespace and
wakes an idle daemon read with EOF. The directory's underlying ext2 contents
become visible again. Unmount succeeds with user file handles still open,
invalidating them: pending and later operations fail with ENOTCONN. The open
file leases retain mount storage until close. A daemon reply racing detach also
receives ENOTCONN, which the daemon may treat as a clean shutdown.
Daemon endpoint close or process exit marks the mount
dead, including when an in-flight read retains the endpoint. Closing a duplicate
of the bound endpoint also disconnects it. Pending callers wake with ENOTCONN; later operations return ENOTCONN
until unmount. A serving daemon must therefore open its endpoint after any
launcher fork, keep one descriptor, and avoid fork/dup or helper processes that
inherit that bound descriptor. NS.4's launcher must leave endpoint creation to
the serving child. This endpoint restriction does not apply to ordinary UFS file
descriptors, which share open descriptions across dup/fork.
The kernel retains mount state independently of the fd, without
retaining a reference that prevents endpoint death detection.

Requests expire after `payload::REQUEST_TIMEOUT_MS` (5 seconds) with EIO.
Expiry does not disconnect a living endpoint. Late and unknown replies cannot
complete a different request. NS.1's request table bounds pending work to 64.

## Messages and operation payloads

Framing is defined in `libs/ufs-abi/src/lib.rs`: packed little-endian fields,
22-byte requests and 16-byte replies, each followed by at most MAX_DATA bytes.
Reply errno is 0 on success or a **negative** POSIX errno with an empty payload
on failure. The framing layer can transport either sign; the bridge uses the
negative convention. No Rust or native C structure is transmitted as a payload.

`u32`, `u64` below are little-endian. A `name` is a u16 byte length followed by
that many non-NUL bytes: 1..NAME_MAX, no slash, neither `.` nor `..`. Names need
not be UTF-8. The kernel resolves dot components and supplies directory dot
entries. Trailing bytes in fixed payloads are invalid. `Attr` is exactly:
node:u64, size:u64, mode:u32 (type and permissions), nlink:u32, uid:u32, gid:u32,
atime_ns:u64, mtime_ns:u64. Times are nanoseconds since the Unix epoch; the test
daemon uses epoch times. ctime may be synthesized from mtime by the bridge.

| Operation | Request header node | Request payload | Successful reply payload |
|---|---|---|---|
| Lookup | parent | name | Attr |
| Getattr | node | empty | Attr |
| Setattr | node | new_size:u64 (8 bytes), or times_mask:u32, atime_ns:u64, mtime_ns:u64 (20 bytes) | Attr after update |
| Readdir | directory | cookie:u64 (0 initially) | repeated (next_cookie:u64, node:u64, mode:u32, name); empty at EOF |
| Open | node | flags:u32 (open ABI flags) | handle:u64 |
| Read | node | handle:u64, offset:u64, count:u32 | raw bytes (empty at EOF), at most count |
| Write | node | handle:u64, offset:u64, raw bytes | written:u32 |
| Create | parent | mode:u32, name | Attr; open follows separately |
| Mkdir | parent | mode:u32, name | Attr |
| Unlink | parent | name | empty |
| Rmdir | parent | name | empty |
| Rename | old parent | new_parent:u64, old_name, new_name | empty; atomically replaces destination |
| Fsync | node | handle:u64 | empty (files and directories) |
| Statfs | any node | empty | seven u64: bsize, blocks, bfree, bavail, files, ffree, namelen |
| Release | node | handle:u64 | empty |
| Parent (opcode 16) | non-root directory | empty | parent_node:u64, name |

Parent returns the directory's current parent and exact entry name, including
renames of an open directory. The mount root is supplied by the kernel and
returns ENOENT to Parent. A directory with no namespace name returns ENOENT.
The bridge uses this query to rebuild cwd and directory-fd ancestry by identity;
serving daemons must implement it for directory-relative operations.

The Setattr timestamp form uses mask bit 0 for atime and bit 1 for mtime.
Other bits are invalid. An unset bit leaves that field unchanged in the daemon;
the corresponding wire value is ignored. The kernel resolves UTIME_NOW and
checks ownership/write permission before sending explicit epoch nanoseconds.

Read data is bounded by MAX_DATA. Write data is bounded by `payload::MAX_WRITE`
(MAX_DATA minus the 16-byte handle/offset prefix); larger user I/O is chunked
by the kernel. Readdir returns complete records fitting MAX_DATA; cookies are
opaque resume positions, not byte offsets. The test daemon enumerates sorted
names with one-based next cookies; enumeration during mutation is unspecified.
Statfs is translated to the native syscall structure, with filesystem type
`payload::MAGIC`, fragment size equal to bsize and unsupported fields zeroed.
The test tree has a 32 MiB byte capacity and 4096 node slots. Unlinked nodes
remain allocated while handles refer to them, including replaced destinations.

## Test daemon synchronization

`/usr/libexec/breenix/memfsd_test DIR [--stall-reads]` mounts an empty tree and
writes `READY` plus newline to stdout after mount succeeds. Its suite starts
with a file in the underlying directory to check hiding and restoration.
With `--stall-reads`, the daemon consumes read requests and emits `READ <id>`
plus newline without replying. It still serves other operations. The death
case waits for that acknowledgement before SIGKILL, so the read is pending
at the time of death. The timeout case leaves it alive and requires EIO.
These acknowledgements travel through a private pipe to the suite, separate from
suite result records. They are test-helper behavior, not kernel test hooks.

## Open descriptions

`UfsFile` shares the daemon handle and cursor across duplicate descriptors and
fork. The kernel reserves a descriptor before a creating or truncating open.
Create returns attributes, followed by Open as in the payload table. An Open
request omits O_TRUNC; the kernel applies the size Setattr after taking ownership
of the returned handle. A failed truncation therefore still releases the handle.
The kernel checks access from the daemon's attributes for existing files.

Reads use at most MAX_DATA reply bytes; writes use at most MAX_WRITE data bytes.
A short reply ends the syscall transfer, including a zero-byte reply. An error
after a transferred prefix returns that prefix. Positioned transfers retain the
shared cursor; pwrite on an O_APPEND description follows Breenix's Linux ABI
append behavior. The kernel serializes append size selection and writes against
other writes and size changes to the same node, including through independent
opens. Other nodes and ordinary opens do not acquire that gate. Create exclusivity
is decided by the daemon's atomic Create operation.

Directory enumeration keeps dot-entry progress separately from the daemon's
opaque cookie. Native dirent64 d_off values contain the daemon's next cookie
without arithmetic encoding. A user-copy failure leaves enumeration progress
unchanged. Files and directories both send Fsync using their opened handle;
fstat and fstatfs fetch daemon replies and propagate their errors.

Final object destruction links preallocated Release work into the mount. A
release waiting for the bounded request table does not wait inside close and
does not disappear when the table is full. Daemon reads drain that work as
capacity opens. Wake delivery uses the bridge's deferred reclaim path, which
avoids scheduler calls in process-manager close contexts. A disconnected or
detached mount discards the close work. The bridge has no mapped-file path:
file-backed mmap refuses a UfsFile descriptor with ENODEV, and namei refuses an
executable from a user mount with EACCES.

## Working directories and directory fds

A UFS directory may be a cwd or a directory fd for openat, unlinkat and fstatat.
The held open description keeps the daemon node alive; Parent reconstructs its
current ancestry after a rename, including getcwd and `..` across the mount root.
Chdir checks search permission; opening a directory for reading checks read
permission. An unnamed directory has no getcwd pathname and returns ENOENT.
