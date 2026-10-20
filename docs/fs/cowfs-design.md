# cowfs: Breenix's copy-on-write filesystem (D1)

Design, 2026-10-20, from main at 718994f6. Sections 2 and 3 are the draft of the normative format document,
`docs/fs/cowfs-format.md`. The `filesystem` effort in `docs/efforts/path.json` carries out section 7, and its suite
manifest is `docs/suites/filesystem.json`. The program this belongs to is [the server plan](../metal/server-plan.md).

**Operator decisions and rulings** (2026-10-20, ET; the server plan records the full list):

- **D1:** Breenix's own copy-on-write B-tree filesystem with Btrfs's properties (atomic commits, metadata and data
  checksums with scrub, snapshots). Not the Btrfs format; recovery is through our own host tools.
- **S2:** filesystem code is architecture-independent where possible; iteration runs on the host harness and ARM64
  QEMU, and the R240 carries the acceptance proofs.
- **S4:** the ext2 partitions stay read-only after migration until two upgrades succeed on the new filesystem, and
  are then reclaimed.
- **D6:** the effort covers each target. x86-64 on the R240 leads, and the catch-up brings ARM64 QEMU, Parallels
  and VMware to the same bar. Each VM gets its own persistent install (S10), so the migration of section 6 runs
  there too.

**Clean-room sources:** CRC-32C (RFC 3720), SipHash-2-4 (Aumasson and Bernstein, 2012), SHA-256 (FIPS 180-4), GPT
(UEFI 2.10), FLUSH CACHE EXT (ATA ACS-3), pax (POSIX.1-2017), and Rodeh's reference-counted copy-on-write B-trees
(ACM TOS 3:4, 2008). No filesystem's or compiler's source is read.

## 1. Groundwork

### 1.1 The filesystem interface

**Today.** `fs/vfs/` is an unused stub. Outside `fs/ext2/`, 502 lines in 47 files name ext2, led by `syscall/fs.rs`
(115), `fs/namei.rs` (55) and `main_aarch64.rs` (43). Open files hold an ext2 `FileHandle` (`kernel/src/ipc/fd.rs:74`),
`namei::Mount` is `{Root, Home}`, and callers use ext2 guards directly (`read_inode` 39 times). 0 of 5 Tier-1 files
name ext2; the 2 Tier-2 mentions are comments.

**The trait.** `vfs::FileSystem: Send + Sync` is object-safe, and each method takes its own locks and returns
`Result<_, Errno>`. Inode numbers are `u64`.

| Group | Operations |
|---|---|
| Mount | `kind`, `root`, `statfs`, `sync`, `read_only`, `max_file_size` |
| Inode | `getattr`; `setattr` (mode, owner, times); `open` (pins the inode, returns `Arc<VNode>`) |
| Names | `lookup`; `readdir(dir, cookie, sink)`; `create(dir, name, NewNode)` for the seven file types; `link`; `remove(dir, name, Dir or NonDir)`; `rename(from, to, NOREPLACE or EXCHANGE)`; `readlink` |
| Data | `read(node, off, buf)`; `write(node, Offset or Append, buf)`; `resize`; `fsync(node, data_only)`; `writeback_pages(node, page, bytes)` |
| Xattrs | `xattr(ino, op)` (Breenix has no xattr syscalls yet) |

The VFS keeps paths, mounts, symlinks, `.` and `..`, and all credential decisions (D5: access checks, sticky
directories, chmod and chown rules, a new node's owner and setgid group). <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 1.1, not a measured result. -->

- **Inodes and open files.** ext2's `LiveInode` becomes the VFS `VNode` (mount, inode, incarnation, size, times,
  `MapState`); dropping the last handle does no I/O (`kernel/src/fs/ext2/live_inode.rs:2`). Open files hold an `Arc<VNode>`
  and a position or getdents cookie; directory entries cross as `DirEnt { ino, kind, name }`.
- **Mounts.** A mount table replaces `namei::Mount`; `/home` and the `/etc` and `/root` generation binds become
  mounts and bind mounts, including the shared directories defined in section 6, retiring `strip_home_prefix`,
  `is_home_path` and `Namespace`. Boot probes formats by magic.
- **mmap and fsync.** `MapState` moves into `VNode`; writeback calls `writeback_pages` (ext2: `write_mapped_range`;
  cowfs: joins the transaction). The lock order is filesystem, PROCESS_MANAGER, VNode table, MapState, frame ledger;
  faults never enter a filesystem (`kernel/src/memory/file_map.rs:27`). ext2's fsync is unchanged. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 1.1, not a measured result. -->
- **Fault injection.** `fs_fault_inject` becomes a trait wrapper, `FaultFs<F>`.

**Staged migration.** ext2 stays the only filesystem, so existing suites gate each stage. A guard,
`tests/fs_backend_boundary_structure.rs`, counts `fs::ext2` references outside `fs/ext2/`; each stage lowers its
ceiling, the last to zero. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 1.1, not a measured result. -->

| Stage | Moves | Gate (ARM64 QEMU and `tools/ext2-host-tests`) |
|---|---|---|
| 0 | `u64` inode numbers and `Errno` above ext2 | files-io, directories |
| 1 | The trait, `Ext2Adapter` and the mount table; conformance tests on ext2 (`tools/vfs-host-tests`) | host |
| 2 | Path resolution, mounts and binds | directories, files-io, processes |
| 3 | open, read, write, lseek, getdents, fstat, fsync, ftruncate | files-io, directories |
| 4 | mkdir, rmdir, unlink, link, symlink, rename, mknod, chmod, chown, utimensat, statfs | directories, files-io, ipc, networking (unix) |
| 5 | mmap and writeback | files-io, memory |
| 6 | Boot mounts, exec, procfs, Unix-socket names, test framework | each suite; guard at 0 |

### 1.2 The buffer cache

`block/cache.rs` replaces each mount's 512 KiB write-through `sector_cache.rs` with one cache for all devices. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 1.2, not a measured result. -->

- **Buffers**, keyed by (device, block), are Clean, Dirty(group), InFlight or Failed. Clean, unpinned buffers are
  evicted LRU within 1/8 of memory (4 MiB–1 GiB). Waiters sleep BlockedOnIO; no I/O runs under
  PROCESS_MANAGER.
- **Misses** pass the owner's `verify` callback before caching (cowfs: CRC, address, generation), else EIO.
- **Write groups.** `Hold` groups (cowfs) are written only when submitted; memory pressure asks for a commit.
  `WriteThrough` groups (ext2) write synchronously, leaving ext2's crash behaviour unchanged.
- **Ordering.** `submit(group)` returns a `Batch` to wait on; `flush(dev)` waits for issued writes, sends FLUSH CACHE
  EXT and returns on completion. No later write reaches the device before that flush.

cowfs file data uses the VNode page cache: dirty data waits for its transaction and gets extents (up to 1 MiB) at
commit.

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
reservation returns ENOSPC with nothing changed. `global_reserve` (total/200, clamped to 16 MiB–1 GiB) serves only
operations that free space (unlink, rmdir, truncation, snapshot deletion, xattr removal); `f_bavail` excludes it and
outstanding reservations. Overwrites need new blocks and can return ENOSPC. A commit that runs out of space is a bug:
the mount turns read-only and the harness fails. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 2.8, not a measured result. -->

### 2.9 Limits, features and versions

| Quantity | Limit |
|---|---|
| Block, node | 4096, 16384 bytes (fixed in v1) |
| Filesystem | 64 MiB to 2^48 blocks |
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

**Writer rules**, invisible in an image, are checked by the harness at the device.
- **W1** No write targets a block live in either of the last two durable commits.
- **W2** A superblock is written only after its generation's writes have completed and been flushed.
- **W3** A superblock is durable only after the flush that follows it.

## 4. Host tools

`kernel/src/fs/cowfs/` is `no_std` and architecture-independent; `tools/cowfs/` builds it on the host, as
`tools/ext2-host-tests` does for ext2.

- **`cowfs-mkfs`** runs the kernel's writer on an image file. `--populate DIR` replaces `mke2fs -d`; `--uuid`,
  `--time` and `--hash-key` make images reproducible; `--extents` lists only the ranges to write, plus both rings.
- **`cowfs-check [--full] IMAGE`** is independent of the writer. Another author writes it from sections 2–3, sharing
  only CRC-32C and SipHash, which are tested against the published vectors.
  - Exit codes: 0 clean, 1 warnings, 4 errors, 8 unreadable.
  - Each invariant has a hand-built image that violates it.
  - It also builds as a Breenix program.
- **`cowfs-extract IMAGE ls|cat|dump|manifest|tar|owners`** reads any root or generation through the kernel's
  reader. `--salvage` rebuilds from valid nodes carrying the uuid and flags files failing CRC checks.
  `scripts/suite-verdict.py` uses `dump` for cowfs diskChecks, as it uses debugfs for ext2.

## 5. Crash testing

**Host harness** (`tools/cowfs-host-tests`):
- **Recording device.** A `BlockDevice` logs writes and flushes and fails the run the moment W1–W3 break.
- **Crash images.** At each flush boundary: the durable image plus every subset of the writes issued since, when
  there are at most 12. Otherwise every prefix, every single omission and 4,096 random subsets. Torn variants keep a
  random prefix of a block's 512-byte sectors, the rest old or garbage. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 5, not a measured result. -->
- **Acceptance.** Each image recovers, passes `cowfs-check --full`, and equals the model after a commit between the
  last fsync-acknowledged one and the last whose superblock write was issued.
- **Model.** A reference POSIX model runs the same random streams (create, sparse write, truncate, link, unlink,
  rename, mkdir, rmdir, symlink, setattr, xattrs, fsync, remount, snapshots, fill to ENOSPC and delete) and is
  compared after every operation and crash. Failing seeds are minimized into regression tests. A fixed-seed corpus
  runs in `cargo test`; `cowfs-torture --hours N` produces milestone evidence. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 5, not a measured result. -->
- **Corruption.** Bit flips in committed images yield errors, never panics, and the checker names the block. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 5, not a measured result. -->

**R240 power cuts, on the test root only.** Test boots get only the test root's GUID, so the fence protects system
and home, which are copied off the machine first.

- **Writer.** A `cowfs` suite case runs a seeded stream, appending each operation's sequence number to `/crash/log`
  in the same transaction and printing `FSYNC <seq> <sha256 of model state>` after each fsync. The kernel reports
  the drive's write-cache setting.
- **One cycle**, under one lease:
  1. A suite boot runs on a cowfs test root.
  2. The BMC cuts power 5–90 s into the stream.
  3. A cold readback uploads the partition; `cowfs-check --full` passes, and the tree equals the model replayed to
     `/crash/log`'s last sequence, which is ≥ the last printed `FSYNC`.
  4. A new no-restage boot recovers the partition in the kernel, runs the guest checker and continues.
- **Aimed cuts.** The fault-test feature `cowfs_commit_window` holds a commit for 2 s after node writes, after the
  first flush, or between rings.
- **Bit flips.** `cowfs_corrupt_inject` flips a data bit, then a node bit: reads return EIO, scrub and the checker
  name the block, and the node flip turns the mount read-only.
- **Proposed evidence:** 12 cycles (8 random, 4 aimed) with 0 acknowledged sequences lost.

## 6. Migrating the persistent install (S4)

1. **Layout.** `install-persistent --add-cowfs` adds GPT entries 5 and 6 after partition 4 (64 GiB cowfs system,
   256 GiB cowfs home, two new type GUIDs). The stub writes both GPT copies and the `cowfs-mkfs --extents` images
   with read-back, before any kernel runs.
2. **Intent format 3** adds `mount <guid> <path> ro|rw` lines, with at most two writable fence ranges.
3. **Migration boot.** The migrator runs from the test root; the ext2 partitions mount read-only and the cowfs pair
   writable. `/sbin/persistent-migrate`:
   - copies the current generation's OS (`/bin /sbin /usr`, including `/usr/share/factory/etc`) and mount points
     into root `gen-<id>`; `/etc` and `/opt` are shared state, not generation contents;
   - copies the active machine state from `/.breenix-shared/{etc,var,opt,srv,tmp,run}` into a shared root
     exposed as `/.breenix-shared` on the cowfs system partition, outside `gen-<id>` (S8 and
     [the users layout](../users/design.md#4-the-standard-filesystem-layout)); copies the existing shared
     `/root` separately. A legacy source must first receive the users layout migration; do not seed accounts,
     host keys or policy from a generation's factory defaults;
   - copies `/home`, including `/home/operator` (S7);
   - preserves bytes, owners, modes, times, symlinks, hard links, devices and FIFOs; all-zero 4 KiB runs become
     holes; <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 6, not a measured result. -->
   - writes a manifest per side (hex path, type, mode, uid, gid, link group, size, mtime, SHA-256), and prints
     `[migrate] manifests equal` only on a match.
4. **Host check.** On a cold readback of all four partitions, debugfs (an independent ext2 reader) and
   `cowfs-extract` (after `cowfs-check --full`) build the two manifests, which must match. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 6, not a measured result. -->
5. **Cutover.** Only then is `NEXT` built with the cowfs GUIDs and `userland <id>` naming `gen-<id>`. A failed
   verification boot rolls back automatically (D3). The persistent mount layout is:

   | Source | Persistent path |
   |---|---|
   | System root `gen-<id>` | OS tree (`/bin`, `/sbin`, `/usr`) and mount points |
   | System shared root `/.breenix-shared/{etc,var,opt,srv,tmp,run}` | Bind each directory at `/etc`, `/var`, `/opt`, `/srv`, `/tmp`, `/run`, respectively |
   | System shared root for root's home | `/root` |
   | Home partition root | `/home` (nosuid,nodev) |

   Mount shared state before running the users design's `etc-reconcile`, then start services. Reconcile
   generation defaults from `/usr/share/factory/etc` into shared `/etc`, preserving machine-owned accounts,
   shadow entries, SSH host keys and admin policy. Upgrades and cowfs generation rollbacks keep these binds
   and the same shared state; selecting an older cowfs generation must not restore old credentials. The frozen ext2 rescue boot below
   uses the pre-migration accounts and keys instead.
6. **Rollback.** The ext2 copy stays frozen and never mounts writable (S4). PREVIOUS becomes the migrating commit
   with both ext2 partitions read-only and a RAM-disk cowfs for `/tmp` and `/run`, a rescue boot with every file
   readable over SSH. Making ext2 writable again is an operator action. <!-- claim-lint:ok: Normative cowfs design requirement from docs/fs/cowfs-design.md section 6, not a measured result. -->
7. **Upgrades on cowfs.**
   - The installer fills root `<id>.new` and renames it `gen-<id>` in one transaction.
   - `home` is snapshotted read-only before each upgrade.
   - Shared `/tmp` is emptied at boot (mode 1777); shared `/run` is emptied by init before services start
     (mode 0755). Their backing directories stay in `/.breenix-shared`, outside generations. `/var/tmp`
     remains in shared `/var` and is not cleared by this boot cleanup.
   - Reclamation (#2117) deletes roots no slot names.
8. **Reclaim.** After two upgrades verified on cowfs, a stub boot removes GPT entries 3 and 4.

## 7. The filesystem effort's milestones

QEMU means ARM64 QEMU; R240 acceptance is a metal landing gate. Each milestone's measure in
`docs/efforts/path.json` is one or two categories of the `filesystem` suite, which has no cases yet: each category's
cases arrive with the lane that implements it. "Evidence" is what those cases and the lane's landing evidence check.
Categories whose evidence is a machine event (a power cut, a migration, a reclaim boot) are host-driven records, as the
SDK suite's host cases are.

| # | Milestone | Measure (`filesystem` suite) | Evidence | Iterate | Accept |
|---|---|---|---|---|---|
| 1 | `fs-interface` | `interface` | Guard at 0; files-io, directories, memory, processes, ipc, networking (unix) | host, QEMU | R240, each suite |
| 2 | `buffer-cache` | `buffer-cache` | Recording-device ordering tests; files-io, directories | host, QEMU | R240, each suite and a cold readback |
| 3 | `format` | `format` | Format doc; mkfs images pass the checker; violating images refused | host | host |
| 4 | `read` | `read` | A host-built disk matches its manifest | host, QEMU | QEMU |
| 5 | `write` | `write` | Model runs; files-io and directories on a cowfs root | host, QEMU | QEMU |
| 6 | `enospc` | `enospc` | Host fill-and-delete | host, QEMU | QEMU |
| 7 | `crash-host` | `crash-images` | Clean crash and corruption corpora; 24-hour torture run | host | host |
| 8 | `checksums-scrub` | `checksum`, `scrub` | Bit flips return EIO; scrub and the checker name the block | host, QEMU | QEMU |
| 9 | `snapshots` | `snapshots` | Snapshots in the model and the crash corpus | host, QEMU | QEMU |
| 10 | `test-root` | `test-root` | Each suite on a cowfs test root | QEMU | R240 |
| 11 | `power-cut` | `power-cut` | Section 5's 12 cycles and bit-flip boots | — | R240 |
| 12 | `persistent-root` | `persistent-root` | Section 6 steps 1–6; rescue rollback and return | host, QEMU | R240 |
| 13 | `ext2-retired` | `ext2-retired` | Two verified cowfs upgrades; reclaim boot | — | R240 |

Milestones 1–2 need little R240 time and 3–9 need no R240 time; 11–12 follow the server effort's reboot, watchdog and rollback
and the users effort's accounts. The suite is named after the effort rather than `cowfs`, because its first two
categories measure the interface and the cache on ext2.

**Catch-up (D6).** After a milestone is accepted on the R240, the same categories pass on ARM64 QEMU, Parallels and
VMware. Power cuts on a VM are a hard stop of the VM process during a commit; that is a weaker test than removing
power from a drive with a volatile write cache, and the R240's cycles remain the power-loss evidence.
