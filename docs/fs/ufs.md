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
become visible again. Unmount after daemon death succeeds when user file
handles have been closed. Daemon endpoint close or process exit marks the mount
dead, waking pending callers with ENOTCONN; later operations return ENOTCONN
until unmount. The kernel retains mount state independently of the fd, without
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
| Setattr | node | new_size:u64 | Attr after truncate |
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
