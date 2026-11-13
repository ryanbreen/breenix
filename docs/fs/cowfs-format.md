# cowfs version 1 format

This document is normative. The words must and shall describe requirements on images and readers. Offsets are byte offsets, ranges are inclusive unless stated otherwise, and all integer fields are unsigned little-endian except signed seconds. Packed fields have no implicit alignment. The source of the format is [cowfs design](cowfs-design.md), sections 2–3. <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->

## 2. On-disk format

Little-endian and packed. A block is 4096 bytes, numbered from the partition start; a node is four blocks (16384
bytes); `total_blocks = floor(partition bytes / 4096)`.

| Blocks | Contents |
|---|---|
| 0–15 | Reserved for the storage record and boot use; cowfs does not write them |
| 16–19 | Superblock ring A, slots 0–3 |
| 20 … total−5 | Nodes and data |
| total−4 … total−1 | Superblock ring B, slots 0–3 |

**Checksums.**
- CRC-32C: reflected polynomial 0x82F63B78, initial value and final XOR 0xFFFFFFFF ("123456789" gives 0xE3069283).
- Superblocks and nodes checksum the bytes after their CRC field. Pointers repeat the node's CRC and generation, so a
  stale, lost or misdirected node write fails at the parent.
- Each data block's CRC sits in its extent's REF item (2.4).
- Names hash with SipHash-2-4 under the superblock key (key 00…0f, message 00…0e: 0xa129ca6149be45e5).

### 2.1 Superblock (one slot, 4096 bytes)

```
off size field
  0   8  magic "CoWFS-SB"
  8   4  csum: CRC-32C of bytes 12..4095
 12   4  csum_type = 1 (CRC-32C)
 16  16  fs_uuid
 32   8  generation (>= 1)
 40   1  ring (0 = A, 1 = B)
 41   1  slot = generation mod 4
 42   2  zero
 44   4  block_size = 4096
 48   4  node_size = 16384
 52   4  zero
 56   8  total_blocks
 64   8  compat     (unknown bits: mount)
 72   8  ro_compat  (unknown bits: read-only only)
 80   8  incompat   (unknown bits: refuse; v1 = bit 0, BASE)
 88  24  space_root (BlockPtr)
112   8  free_blocks
120   8  global_reserve (blocks)
128   8  next_root_id
136  16  dirhash_key
152  16  commit_time (i64 s, u32 ns, 4 zero)
168  16  mkfs_time
184   8  last_scrub_generation (0 = never)
192   8  last_scrub_errors
200  64  label (UTF-8, NUL padded)
264  32  writer (ASCII, e.g. "breenix 718994f6")
296      zero to 4095

BlockPtr (24): addr u64, generation u64, csum u32, level u8, 3 zero
```

### 2.2 Nodes and keys

```
Node header (48)                     Key (17): object u64, type u8, offset u64,
  0  8  magic "CoWFS-ND"               ordered by (object, type, offset)
  8  4  csum: CRC-32C of 12..16383
 12  1  tree (1 = FS, 2 = SPACE)     Leaf item header (21): key, data_off u16
 13  1  level (0 = leaf)               (from node start), data_len u16
 14  2  nitems
 16 16  fs_uuid                      Internal entry (37): key, child addr u64,
 32  8  addr (own block number)        child generation u64, child csum u32
 40  8  generation
```

Leaf item headers follow the node header in key order; payloads fill from the node's end without overlap; unused
bytes are zero. Internal nodes hold at most 441 entries, and entry *i*'s key is ≤ each key in child *i* and greater
than each key in child *i−1*. Payloads are at most 6144 bytes; trees are at most 8 levels deep. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.2, not a measured result. -->

### 2.3 The filesystem tree (one per root)

| Type | Key (object, type, offset) | Payload |
|---|---|---|
| INODE 0x01 | (ino, 0x01, 0) | 128-byte inode |
| DIR_ENTRY 0x10 | (dir, 0x10, SipHash(name)) | records; several only when names collide |
| DIR_INDEX 0x11 | (dir, 0x11, index ≥ 2) | one record; readdir order and cookie |
| EXTENT 0x20 | (ino, 0x20, byte offset, 4 KiB aligned) | 24-byte extent |
| INLINE 0x21 | (ino, 0x21, 0) | symlink target, 1–4095 bytes |
| XATTR 0x30 | (ino, 0x30, SipHash(name)) | records: `name_len u8, value_len u16, name, value` |
| ORPHAN 0x40 | (0xFFFF_FFFF_FFFF_FF00, 0x40, ino) | empty |

```
Inode (128)                            Directory record (10 + n)
  0  4 mode (st_mode with setuid,        0 8 child ino
       setgid, sticky)                   8 1 kind: 1 reg, 2 dir, 3 chr, 4 blk,
  4  4 nlink                                 5 fifo, 6 sock, 7 symlink
  8  4 uid         12 4 gid              9 1 name_len (1..255)
 16  8 size                             10 n name (no '/' or NUL; not . or ..)
 24  8 nblocks (blocks mapped)
 32  8 parent (dirs; root: itself)     Extent (24)
 40  8 rdev                              0 8 phys_start (block)
 48  8 next_index (dirs; from 2)         8 4 phys_len (blocks of the REF)
 56  8 created_generation               12 4 extent_offset (blocks into it)
 64 12 atime (i64 s, u32 ns)            16 4 num_blocks (>= 1)
 76 12 mtime     88 12 ctime            20 4 flags = 0
100 12 btime    112 16 zero
```

The root directory is inode 2; other inode numbers start at 16 and are not reused within a root. Directories store
no `.` or `..` (`..` is `parent`), and their size is their entry count. Holes have no extent; bytes past `size` in a
file's last mapped block are zero. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.3, not a measured result. -->

### 2.4 The space tree (one per filesystem)

| Type | Key | Payload |
|---|---|---|
| REF 0x80 | (start block, 0x80, length) | `refcount u32`, `kind u8` (1 FS node, 2 space node, 3 data), `3 pad (0)`, `alloc_generation u64`; data adds `length × u32` CRC-32C |
| DEAD 0x83 | (node addr, 0x83, 4) | empty |
| ROOT 0x90 | (0xFFFF_FFFF_FFFF_FF10, 0x90, root id) | below |
| FREE 0x81 | (0xFFFF_FFFF_FFFF_FF20, 0x81, start) | `length u64` |
| PENDING 0x82 | (0xFFFF_FFFF_FFFF_FF30, 0x82, start) | `length u64, released_generation u64` |

```
Root (65 + n)
  0 24 root (BlockPtr to an FS tree)     48 12 ctime
 24  8 next_inode                        60  4 flags: bit 0 READ_ONLY
 32  8 source_root (0: created empty)    64  1 name_len (1..255)
 40  8 created_generation                65  n name
```

Data REFs cover 1–256 blocks and node REFs 4. DEAD marks an unreferenced FS node whose children still count as
referenced. Free space has its own key range, so a mount reads only FREE and PENDING items.

### 2.5 Transactions, commit and recovery

Each modifying syscall belongs to exactly one in-memory transaction, closed by fsync-class calls, after 5 s, at 64
MiB of dirty data, or under reservation pressure. A node written by an earlier transaction is never
changed: the first change copies it and its ancestors to new addresses. Following Rodeh, copying a node with refcount
1 releases the old copy, and copying a shared node decrements it and increments each child (for a leaf, each data
extent its EXTENT items name). <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.5, not a measured result. -->

**Reuse rule.** A block released by commit *G* is not overwritten until superblock *G+1* is durable. It becomes
PENDING with `released_generation = G`, and transaction *T* may allocate PENDING blocks with
`released_generation ≤ T−2` (converting them to FREE). So the last two durable commits stay intact while a commit is
written.

**Commit of *G*** (each mount's `cowfs-commit` kthread):
1. Under the write lock, apply the deferred refcounts, releases, PENDING conversions and ROOT updates to the space
   tree until nothing changes. This ends because a node is copied at most once per transaction. Then open *G+1*. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.5, not a measured result. -->
2. Fill node headers bottom-up (address, generation *G*, CRC).
3. Write *G*'s nodes and data, in any order, and wait for completion.
4. FLUSH.
5. Write superblock *G* to slot *G mod 4* of rings A and B.
6. FLUSH. *G* is durable, and its fsync waiters succeed.

A failure in steps 3–6 writes no superblock, returns EIO and turns the mount read-only; *G−1* is intact.

**Recovery** reads the eight slots.
1. A slot is valid when its magic, CRC, `csum_type`, sizes, ring, slot (= generation mod 4), incompat bits and
   `total_blocks` (fits the partition) check out.
2. Valid slots that disagree on `fs_uuid` refuse the mount.
3. *M* is the highest valid generation whose space root, and whose ROOT tree roots written in that generation, match
   their pointers (magic, CRC, address, generation, level, uuid, tree).
4. A writable mount zeroes valid slots above *M*, and flushes, before its first commit, so an abandoned commit is
   never chosen later. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.5, not a measured result. -->

There is no log to replay; orphan reclaim and DEAD reaping resume. Image writers write all eight slots, zeroing
unused ones, so a stale superblock from an earlier image never wins. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.5, not a measured result. -->

**fsync.** `fsync`, `fdatasync`, `syncfs`, `sync`, `msync(MS_SYNC)` and `O_SYNC`/`O_DSYNC` writes return after step
6. A crash recovers exactly one commit's state, no earlier than the last acknowledged fsync. That is stronger than
POSIX: syscalls are all-or-nothing, and write-then-rename cannot leave an empty file. After a failed commit, fsync
returns EIO. Mapped pages are copied to a bounce buffer before checksumming. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.5, not a measured result. -->

### 2.6 Snapshots

A snapshot is a ROOT item that points at another root's tree root and increments its refcount; without `READ_ONLY`
it is a writable clone. Deletion removes the ROOT item and decrements the tree root; a node
reaching zero becomes DEAD. The reaper takes one DEAD node per step, decrements its children or named extents (nodes
reaching zero become DEAD, extents PENDING), then makes it PENDING. Each commit is consistent mid-deletion, so
deletion is O(1) to request and resumes after a crash. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.6, not a measured result. -->

### 2.7 Scrub

A `cowfs-scrub` kthread (ioctl-controlled) walks REF and DEAD items in address order at 50 MB/s by default,
re-descending from the current root each batch, and checks node and data CRCs. The next commit records
`last_scrub_generation` and `last_scrub_errors`; `cowfs-extract owners <block>` names affected files. Reads verify
too: bad data returns EIO, and a bad node returns EIO and turns the mount read-only. v1 keeps one copy, so it detects
without repairing; incompat bit `META_DUP` is reserved for duplicated metadata.

### 2.8 ENOSPC reservation

Before changing anything, an operation reserves its data blocks, its paths' FS-tree nodes not yet copied in this
transaction, and one split per level. The transaction also holds space-tree nodes for its space-item changes (three
per allocated node or extent), at most min(changes × (space height + 1), 2 × space nodes + space height). A failed
reservation returns ENOSPC with nothing changed. `global_reserve` (clamp(floor(total_blocks/200), 4096, 262144) blocks) serves only
operations that free space (unlink, rmdir, truncation, snapshot deletion, xattr removal); `f_bavail` excludes it and
outstanding reservations. Overwrites need new blocks and can return ENOSPC. A commit that runs out of space is a bug:
the mount turns read-only and the harness fails. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.8, not a measured result. -->

### 2.9 Limits, features and versions

| Quantity | Limit |
|---|---|
| Block, node | 4096, 16384 bytes (fixed in v1) |
| Filesystem | 16384 to 2^48 blocks |
| File | 2^63 − 4096 bytes |
| Name, symlink target, xattr name, xattr value | 255, 4095, 255, 4096 bytes |
| Refcount | 2^32 − 1 (overflow: EMLINK) |
| Inode numbers, root ids | 64-bit, not reused |

The feature masks are the version; v1 sets only incompat bit 0. A change a v1 reader would misread takes a new
incompat bit; new superblock fields use the zero area behind a bit; the checker and extractor refuse unknown incompat
bits. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.9, not a measured result. -->

## 3. Invariants

The writer keeps these at each commit; `cowfs-check` reports violations by number. Abandoned slots and nodes under a
quarter full are only warnings.

**Superblocks.**
- **S1** Slots are valid or invalid by 2.5's test; ring B's position follows from `total_blocks`.
- **S2** Valid slots share `fs_uuid`; a generation's A and B copies differ only in `ring` and CRC.
- **S3** *M* is recovery's choice; valid slots above it are abandoned.
- **S4** `free_blocks` is the sum of FREE lengths; `next_root_id` exceeds each root id; `global_reserve` follows 2.8.

**Nodes.**
- **N1** Each BlockPtr's addr, generation, CRC and level match its node, as do the node's CRC and uuid; ROOT items
  lead to FS trees and `space_root` to SPACE.
- **N2** A child's generation is ≤ its parent's; all generations are ≤ *M*. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 3, not a measured result. -->
- **N3** Levels fall by one to 0; height ≤ 8; only a root leaf may be empty.
- **N4** Keys strictly increase within a node and stay inside the parent's bounds (2.2).
- **N5** Payloads lie inside the node, do not overlap and are ≤ 6144 bytes; unused bytes are zero; internal nodes
  have ≤ 441 entries. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 3, not a measured result. -->
- **N6** Each tree holds only its own item types; fixed-size payloads have their exact size.

**Space.** L is the set of nodes reachable from `space_root`, ROOT items' roots and DEAD nodes, once per address.
- **A1** Blocks 0–19 and ring B are reserved; each other block lies in exactly one REF, DEAD, FREE or PENDING extent.
- **A2** No two FREE extents are adjacent.
- **A3** Each non-DEAD node in L has a REF of length 4 and matching kind. Each EXTENT names a data REF of the same
  start and length; data REFs cover 1–256 blocks.
- **A4** refcount(X) equals the references to X from `space_root`, ROOT items, internal entries of nodes in L, and
  EXTENT items in leaves in L (one per item).
- **A5** No REF has refcount 0; space nodes have refcount 1; nothing references a DEAD node. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 3, not a measured result. -->
- **A6** PENDING `released_generation` values are ≤ *M*.

**Checksums.**
- **C1** A data REF holds `length` CRCs; in full mode each data block matches its CRC.

**Filesystem trees**, per root:
- **F1** Inode 2 is a directory and its own parent.
- **F2** Each object except the orphan object has an INODE item at its lowest key.
- **F3** DIR_ENTRY records sit in directories, have valid names unique per item at offset SipHash(name), and name
  existing inodes of matching kind.
- **F4** Per directory, DIR_INDEX and DIR_ENTRY records correspond one to one; indexes lie in [2, `next_index`).
- **F5** A non-directory's nlink equals the entries naming it, and is 0 only with an ORPHAN item. A directory's nlink
  is 2 plus its subdirectories; each directory but inode 2 is named by exactly one entry, in its `parent`.
- **F6** Every directory's parent chain reaches inode 2. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 3, not a measured result. -->
- **F7** A regular file's extents are aligned, disjoint and start below `size` rounded up to 4 KiB, with
  `extent_offset + num_blocks ≤ phys_len` and flags 0; `nblocks` is their sum. In full mode, bytes past `size` are
  zero. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 3, not a measured result. -->
- **F8** A symlink has one INLINE item of length `size` and no extents. Directories, devices, FIFOs and sockets have
  neither; only directories have nonzero size (their entry count).
- **F9** Xattr names are unique, at offset SipHash(name), within limits.
- **F10** Inode numbers are below `next_inode`; ROOT names are unique and valid; `mode` holds one of the seven types.

**Writer rules**, invisible in an image, are harness-only and are checked at the device, not by the image checker.
- **W1** No write targets a block live in either of the last two durable commits.
- **W2** A superblock is written only after its generation's writes have completed and been flushed.
- **W3** A superblock is durable only after the flush that follows it.

## 4. Packed field tables

The following tables specify field offsets within each structure or item payload. Fields named zero or pad must be zero. Times have signed 64-bit seconds followed by unsigned 32-bit nanoseconds in 0..999999999. A 16-byte time adds four zero bytes. Node and data addresses are block numbers relative to the partition, not byte offsets. <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->

### Superblock and pointer

| Offset | Bytes | Field |
|---:|---:|---|
| 0 | 8 | magic `CoWFS-SB` |
| 8 | 4 | CRC-32C over bytes 12..4095 |
| 12 | 4 | csum_type = 1 |
| 16 | 16 | fs_uuid |
| 32 | 8 | generation, at least 1 |
| 40 | 1 | ring: A=0, B=1 |
| 41 | 1 | slot, generation mod 4 |
| 42 | 2 | zero | <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->
| 44 | 4 | block_size = 4096 |
| 48 | 4 | node_size = 16384 |
| 52 | 4 | zero | <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->
| 56 | 8 | total_blocks |
| 64 | 8 | compat |
| 72 | 8 | ro_compat |
| 80 | 8 | incompat: BASE = bit 0 |
| 88 | 24 | space_root BlockPtr |
| 112 | 8 | free_blocks |
| 120 | 8 | global_reserve |
| 128 | 8 | next_root_id |
| 136 | 16 | dirhash_key |
| 152 | 16 | commit_time |
| 168 | 16 | mkfs_time |
| 184 | 8 | last_scrub_generation |
| 192 | 8 | last_scrub_errors |
| 200 | 64 | UTF-8 label, NUL padded |
| 264 | 32 | ASCII writer |
| 296 | 3800 | zero | <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->

| BlockPtr offset | Bytes | Field |
|---:|---:|---|
| 0 | 8 | addr |
| 8 | 8 | generation |
| 16 | 4 | csum |
| 20 | 1 | level |
| 21 | 3 | zero | <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->

### Node structures

| Node header offset | Bytes | Field |
|---:|---:|---|
| 0 | 8 | magic `CoWFS-ND` |
| 8 | 4 | CRC-32C over bytes 12..16383 |
| 12 | 1 | tree: FS=1, SPACE=2 |
| 13 | 1 | level, leaf=0 |
| 14 | 2 | nitems |
| 16 | 16 | fs_uuid |
| 32 | 8 | own addr |
| 40 | 8 | generation |

| Key offset | Bytes | Field |
|---:|---:|---|
| 0 | 8 | object |
| 8 | 1 | type |
| 9 | 8 | offset |

| Leaf header offset | Bytes | Field |
|---:|---:|---|
| 0 | 17 | key |
| 17 | 2 | data_off from node start |
| 19 | 2 | data_len |

| Internal entry offset | Bytes | Field |
|---:|---:|---|
| 0 | 17 | key |
| 17 | 8 | child addr |
| 25 | 8 | child generation |
| 33 | 4 | child csum; child level is parent level minus one |

### FS payloads

| INODE offset | Bytes | Field |
|---:|---:|---|
| 0 | 4 | mode |
| 4 | 4 | nlink |
| 8 | 4 | uid |
| 12 | 4 | gid |
| 16 | 8 | size |
| 24 | 8 | nblocks |
| 32 | 8 | parent |
| 40 | 8 | rdev |
| 48 | 8 | next_index |
| 56 | 8 | created_generation |
| 64 | 12 | atime |
| 76 | 12 | mtime |
| 88 | 12 | ctime |
| 100 | 12 | btime |
| 112 | 16 | zero | <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->

DIR_ENTRY payloads consist of consecutive directory records; DIR_INDEX contains exactly one. A directory record has:

| Offset | Bytes | Field |
|---:|---:|---|
| 0 | 8 | child ino |
| 8 | 1 | kind: 1 regular, 2 directory, 3 character, 4 block, 5 FIFO, 6 socket, 7 symlink |
| 9 | 1 | name_len, 1..255 |
| 10 | name_len | name bytes; no slash, NUL, `.` or `..` |

| EXTENT offset | Bytes | Field |
|---:|---:|---|
| 0 | 8 | phys_start |
| 8 | 4 | phys_len |
| 12 | 4 | extent_offset |
| 16 | 4 | num_blocks |
| 20 | 4 | flags, zero | <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->

INLINE contains exactly the symlink target bytes, 1..4095 bytes. ORPHAN has a zero-byte payload. XATTR consists of consecutive records:

| Offset | Bytes | Field |
|---:|---:|---|
| 0 | 1 | name_len, 1..255 |
| 1 | 2 | value_len, 0..4096 |
| 3 | name_len | name bytes |
| 3 + name_len | value_len | value bytes |

### SPACE payloads

| REF offset | Bytes | Field |
|---:|---:|---|
| 0 | 4 | refcount |
| 4 | 1 | kind: 1 FS node, 2 SPACE node, 3 data |
| 5 | 3 | zero | <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->
| 8 | 8 | alloc_generation |
| 16 | 4 × length for data, otherwise 0 | CRC-32C per block in ascending block order |

DEAD has a zero-byte payload. FREE has an 8-byte `length` at offset 0. PENDING has an 8-byte `length` at offset 0 and an 8-byte `released_generation` at offset 8.

| ROOT offset | Bytes | Field |
|---:|---:|---|
| 0 | 24 | FS root BlockPtr |
| 24 | 8 | next_inode |
| 32 | 8 | source_root; zero for a root created empty | <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->
| 40 | 8 | created_generation |
| 48 | 12 | ctime |
| 60 | 4 | flags: READ_ONLY = bit 0 |
| 64 | 1 | name_len, 1..255 |
| 65 | name_len | name |

## 5. Checker interface and recovery

`cowfs-check [--full] IMAGE` writes one finding per stdout line: `error <ID> <detail>` for an invariant violation, or `warning <detail>` for a warning. The ID is the invariant identifier from section 3. Exit status is exit 0 for a clean image, exit 1 for warnings only, exit 4 for any error, and exit 8 if the file is unreadable or no valid slot exists. Exit 8 takes precedence over the other statuses. Full mode checks data CRCs and the zero tail in F7; the ordinary mode checks the remaining image invariants. W1–W3 are harness-only and cannot be established from an image. <!-- claim-lint:ok: Normative format requirement; docs/fs/cowfs-design.md sections 2–3. -->

Recovery reads slots A at blocks 16..19 and B at `total_blocks-4..total_blocks-1`. A slot passes the section 2.5 validity test only if its physical ring and slot match its fields and its generation, the supported format and the partition bounds. Disagreement of UUID among valid slots refuses recovery. For each candidate generation, validate the SPACE root against its pointer, then the ROOT tree roots written in that generation against their pointers; choose the highest candidate passing those checks as M. A valid higher slot is abandoned and is a warning; before a writable mount's first commit it is zeroed and flushed. If there is no valid slot, the checker exits 8. A node less than one quarter full is a warning. <!-- claim-lint:ok: Recovery and warning requirements are specified in docs/fs/cowfs-design.md sections 2.5 and 3, not measured results. -->
