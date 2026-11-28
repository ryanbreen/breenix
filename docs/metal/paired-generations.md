# Paired kernel and system-root generations on the R240

Design, 2026-11-27, from main at b22a066f, for ruling DEV-40 and issue #3263. The ruling's order is: this
design PR, then two implementation PRs (section 9), then the live R240 run that closes PR B. E6 PR 4 resumes
after that.

**The rule.** A deploy is a pair: a kernel slot and a complete system root. The two are selected together, tried
once together, committed together and rolled back together. Each system root is a BreenFS root (a snapshot). A
new one starts as a writable clone of the running root, so changes made in place on the box (DEV-2, DEV-10) carry
forward. The new userspace is written into the clone from the running system, as root over SSH, with checksums.
`/home` is not part of a pair and the deploy does not write to it.

**Sources.** Our own code and docs only: `kernel/src/fs/breenfs/`, `docs/fs/breenfs-design.md`,
`docs/fs/breenfs-format.md`, `docs/fs/migration-manifest.md`, `userspace/programs/src/persistent_install.rs` and
its `breenfs.rs` and `local.rs`, `kernel/src/fs/vfs/boot.rs`, `kernel/src/block/boot_mount.rs`,
`kernel/src/boot/intent.rs`, `libs/libbreenix/src/layout.tsv`, `scripts/metal/metal_live_flash.py`,
`scripts/metal/metal_persist.py`, `scripts/metal/efi-stub/src/{live_flash,live_sequence,breenfs_files}.rs` and
`manager/`, `scripts/devbox/README.md` and `docs/metal/README.md`. External references are specifications and
papers only: UEFI 2.10 (the file and block I/O protocols), FIPS 180-4 (SHA-256), and Rodeh's reference-counted
copy-on-write B-trees (ACM TOS 3:4, 2008), which BreenFS already follows. No other OS, filesystem or libc source
was read.

**On-disk format.** This design needs no BreenFS on-disk format change. Section 2.6 of the format already defines a
writable clone: a ROOT item without `READ_ONLY` that shares another root's tree. Everything below is new in-memory
state, one new ioctl and one new mount source.

## 1. What the code does today

| Piece | Where | State |
|---|---|---|
| Writable clone of the mounted root | `Files::snapshot(name, read_only=false)`, `kernel/src/fs/breenfs/ops.rs:697`; ioctl `SNAPSHOT_CREATE` flags 0, `vfs.rs:3352` | Present. It syncs first, then waits until the generation is fresh with no commit pending, so the clone shares the committed tree rather than uncommitted edits. |
| Empty root, rename, delete, list | `create_empty_root` (ops.rs:721), `rename_root` (748), `delete_root` (769); `SNAPSHOT_RENAME`, `SNAPSHOT_DELETE`, `SNAPSHOT_NEXT` | Present. Rename refuses the booted root and root 1. Delete refuses the booted root, root 1, and any root a mount has claimed (`check_root_unmounted`, vfs.rs:1793). Deletion is O(1) to request, and the DEAD reaper finishes it across commits (format 2.6). |
| Mounting a named root | mount data `root=NAME` (`mount_args::parse_root`, mount_args.rs:37); `BreenFs::mount_named_cached` | Present, but only as a mount that owns the volume. A second writable mount of a volume that already has a writer fails with `EBUSY` (`WritableOwner::claim`, vfs.rs:266). |
| Booting a named root | intent line `userland <id>` (intent.rs:94) → `prepare_mounts` gives it to the writable BreenFS `/` (vfs/boot.rs:105) → `mount_cutover` mounts `gen-<id>` with `mount_persistent_cached` (vfs/boot.rs:167, vfs.rs:1745) | Present. One writer serves the booted root and the default tree (root 1). The shared binds `/etc /var /opt /srv /tmp /run` come from `/.breenix-shared` and `/root` from the default tree. `/proc/mounts` shows `/` with source `gen-<id>`. |
| Reading a non-booted root on the live writer | `GenerationView` (vfs.rs:4463), `Files::with_read_root` (ops.rs:817) | Present, read-only: its mutation entry points return `EROFS`. |
| Pairing kernel and root | The slot image's `ramdisk` intent names `userland <id>` (`metal_persist.slot_image`, metal_persist.py:252). The installer's header states the rule: no independently mutable pointer can pair a kernel with the wrong userland (persistent_install.rs:1-3). | Present. CURRENT, PREVIOUS, NEXT and TRY each carry their root's name inside the image, and the image's SHA-256 seals it. |
| One-reboot kernel flash | `metal_live_flash.py`; the manager's `live_sequence::process` (live_sequence.rs); records in `/root/.breenix-flash` on the default tree, read by the manager's BreenFS reader (`breenfs_files.rs`) | Present (#3247). It reuses CURRENT's userland ID (metal_live_flash.py:100), so both slots name the same root. |
| Generation install from a clone | `Upgrade::install` (persistent_install/breenfs.rs:70) | Present, but only on the netbooted install path. A test root holds the new tree, `/migrate/system` is the only writer, and the installer remounts that volume with `root=` (breenfs.rs:244). It clones CURRENT, moves the old tree aside inside the clone, copies `bin sbin usr`, carries local changes (`local.rs`), then renames `<id>.new` to `gen-<id>`. Its copy list is `bin sbin usr`, without `/lib`. |
| Local-change detection | `.breenix-image` baseline (`local::record`, local.rs:91), `LocalFiles::inspect` and `apply` | Present. If the new image owns a path that was changed on the box, the new image takes the live path and the on-box version is kept under `.breenix-carried/<previous>/` with set-ID bits cleared (local.rs:218-268). |
| Devbox toolchain | `/opt/breenix/rust/<id>/` and `/opt/breenix/rust/CURRENT` (scripts/devbox/README.md); runtime in `/lib` | `/opt` is shared state. A root rollback therefore leaves `CURRENT` naming the new toolchain. |

**The gap, from the code.**
1. A running system cannot write into a root it did not boot. That is the only BreenFS capability this design
   needs that is missing (section 3).
2. The live flash builds its slot from CURRENT's userland ID, so no deploy pairs a new kernel with a new root.
3. The clone installer runs only from a netbooted test root. The tools refuse it once CURRENT is writable
   (metal-boot:1112). It also omits `/lib` and the toolchain selection.
4. The toolchain selection lives in shared `/opt`.

## 2. Terms

- **Generation.** A BreenFS root named `gen-<id>` on the system volume, with `<id>` the deploy's build ID
  (`YYYYMMDDTHHMMSSZ-metal`, the shape `is_metal_generation` already reclaims). It is writable (flags 0). The booted
  generation changes in place like any pet root. A generation that is not booted is not mounted.
- **Draft.** A root named `<id>.new` while its deploy writes into it. No slot ever names a draft.
- **Pair.** A kernel slot image plus the generation its sealed intent names.
- **Payload.** The paths a generation's userspace owns: `/bin`, `/sbin`, `/lib`, `/usr`, plus the toolchain
  selection file `/usr/lib/breenix/rust-current` (section 4.5). Everything else in a generation's tree (mount points,
  root-level files placed by hand) is the box's, and the deploy carries it over unchanged.
- **Inventory.** The publisher's list of the payload: rows in the canonical migration-manifest TSV
  (`docs/fs/migration-manifest.md`). Each row is hex path, type, mode, uid, gid, link group, size, mtime and SHA-256.
  Link groups are computed over the inventory's rows, so BusyBox's hard links across `/bin` and `/sbin` are one group.
  For E6 the inventory is the "expected install inventory" its publisher emits (toolchain/targets/e6/README.md). For
  a legacy build of main, the metal tools encode it from the built userland tree.
- **Retained set.** The roots that a slot image the manager could still boot names. It is the union of:
  - the booted root (the `/` source in `/proc/mounts`);
  - the roots named by the physical CURRENT and PREVIOUS in `disk.json` `slots`. The host changes those entries
    only when it reads the manager's `live flash: promoted <token>` line (metal-boot `on_manager_line` calls
    `metal_live_flash.promoted`), so they describe the ESP as the manager last reported it;
  - the root of each live-flash attempt whose promotion or rollback the host has not yet read from the serial.

  An acknowledged trial is not assumed to promote. `live_sequence::promote` returns without promoting when the
  base note no longer matches, when a foreign `NEXT` is staged, or when `TRY.IMG` is missing, and `process` then
  reports `Outcome::Current` with the old slots in place. So while a committed trial runs unpromoted, the retained
  set holds the trial, the physical CURRENT and the physical PREVIOUS: three roots, not two.

## 3. Kernel and BreenFS additions (PR A)

### 3.1 A staging tree on the live writer

`Files` (ops.rs) already serves two trees in one transaction. Inode numbers carry a tag (`DEFAULT_TREE = 1 << 63`,
ops.rs:421). `tag`, `tree`, `tree_mut`, `inode_key` and `namespace_ino` route each operation to the booted root or
to the default tree. PR A adds a third slot:

- `STAGING_TREE = 1 << 62`, and `staging: Option<(u64, Root)>` beside `default`.
- The tag becomes the two-bit field `ino & (DEFAULT_TREE | STAGING_TREE)`, with exactly three valid values (0,
  default, staging). `tree` and `tree_mut` choose among the three. `namespace_ino` refuses a local inode at or above
  `1 << 62` in each of the three trees. `include_default_tree`'s `next_inode >= DEFAULT_TREE` check tightens to
  `1 << 62`.
- `include_staging_tree(id)` mirrors `include_default_tree` (ops.rs:546). It refuses root 1, the booted root,
  a `READ_ONLY` root and a second staging tree. It loads that tree's ORPHAN items into the orphan sets, tagged.
  `exclude_staging_tree()` requires no staged orphans still pending reclaim and no dirty pages for staged inodes.
- Commit: `prepare_commit` (ops.rs:1902) writes the staging ROOT item back with `update_root` and marks it changed,
  as it does for root 1. `next_generation` (ops.rs:673) refreshes the staging root from the persisted ROOT item.
  `fork` (ops.rs:634) copies it, so N+1's scratch view and the replay onto N's published view see the same three
  trees. Replay needs no further change, because `Change` records hold tagged inode numbers.
- `metadata_estimate` counts the deepest of the three trees and adds three space changes for each extra tree, as it
  does today for the default tree.

`BreenFs` (vfs.rs) gains `attach_staging(name) -> Result<u64, Errno>` and `detach_staging()`:

- **Attach** takes the device's `selection` lock, then repeats `snapshot_request`'s quiescence loop: sync, then
  require `fresh`, no `pending` and no `next_files`. It then looks the name up in `state.roots`, claims
  `MountedRoot` for it (so delete and rename of that root return `EBUSY`), calls `include_staging_tree`, and
  returns the staging root inode `STAGING_TREE | 2`. Refusals: `ENOENT` (no such root), `EROFS` (a read-only root or
  a read-only writer), `EBUSY` (the booted root, root 1, or a staging tree already attached).
- **Detach** runs only after the staging mount has left the registry (3.2), so no path reaches the tree and no
  reference of the unmount's own walk is outstanding. It syncs, so the staging tree's changes are in a durable
  generation, requires that no node of the staging tag is still referenced (an open file description that
  outlived the mount), then excludes the tree and drops the claim. A refusal at that point leaves the tree
  attached and claimed with no mount: the draft cannot be deleted or renamed until a reboot drops the attachment,
  and `paired abort` reports that. That direction loses no data.
- The staging attachment has no on-disk record. After a crash or reboot, no tree is attached, and the draft is
  just a root.

### 3.2 The mount interface

The mount syscall (`kernel/src/fs/breenfs/mount.rs:12`) gains one source, alongside `boot`: `root-volume`. A call
`mount("root-volume", <dir>, "breenfs", 0, "root=<name>")` by euid 0:

- finds the mount at `/`, which must be a writable `BreenFs`. Otherwise it returns `ENODEV`.
- calls `attach_staging(name)`.
- registers a bind-style `MountPoint`: `Backend::Fs(writer)`, root inode `STAGING_TREE | 2`, `bind = Some((root
  mount id, staging root))`, custody of the covered directory and source `<name>`. This is the shape `mount_cutover`
  already uses for `/etc` and the other shared binds (vfs/boot.rs:203-230).
- refuses flags 1 with `EINVAL`. The existing `GenerationView` already gives read-only access.

Unmount (`mount.rs:90`) checks for this mount before its existing branches, in this order:
1. `registry::unmount(id, force)` removes the entry. `MountTable::remove` applies its existing busy check (pins
   from open files and working directories in the mount) first, so a busy mount is refused before any change:
   still registered, still attached.
2. The syscall drops its resolved walk.
3. `writer.detach_staging()` (3.1).

It does not call `shutdown` on the shared writer. Detaching before removal would be the wrong order: a removal
refused afterwards would leave a registered mount over a tree that is gone.

**Who can reach the draft.** Once mounted, a path through the mount point is checked against the draft's own
root directory, which is the clone of `/` at 0755, so the covered directory's mode no longer applies. The mount
point therefore sits inside a directory that stays protected: `/run/breenix/paired/stage`, where
`/run/breenix/paired` is created by `paired begin` (4.0's walk) as root:root 0700 and refused if it already exists
with another owner, mode or type. Traversal needs search permission on that parent, which only root has.

The x86 write fence needs no change. The staging tree lives on the system partition, whose range the persistent
boot already opened.

### 3.3 The root epoch

A clone captures the running root at one instant. The running root stays writable afterwards, so a delivery made
after the clone would exist only in the old generation. PR A adds `ROOT_EPOCH` (`0xb824_1406`, next to
`SNAPSHOT_NEXT` in `abi.rs`), an ioctl on any file in the mount that returns a `u64`:

- The epoch increases with each change the booted tree (tag 0) admits: create, link, unlink, rename, write,
  truncate, setattr and xattr changes.
- It does not change for atime-only touches (`record_touches`), for writes to the default tree (logs in `/var`,
  `/tmp`, `/run`), or for writes to the staging tree.
- It is in memory only and restarts at 0 on each mount.

The installer reads the epoch before it clones (E1). An unchanged value later means the running root had no change
after the clone, because the clone contains the running root as of E1. A change in between is reported
conservatively, as a refusal.

A last read alone leaves a window: an edit admitted after it and before the reboot would land only in the old root,
the new pair would commit without it, and reclamation would later delete it. PR A therefore adds two more ioctls
beside `ROOT_EPOCH`, for euid 0 only:
- `ROOT_FREEZE` sets an in-memory `frozen` flag on the booted tree and returns the epoch at that instant (E2).
  The flag is checked at the same admission point that advances the epoch, under the same lock, so each change is
  either counted in E2 or refused. While frozen, each change the epoch counts returns `EROFS`. Atime touches, the
  default tree and the staging tree are not frozen. Logs in `/var` and the flash records in `/root` keep working.
- `ROOT_THAW` clears the flag. Only `paired abort` calls it. A reboot also clears it, because it is in memory.

Publish (4.6) freezes the running root and compares E2 with E1. From then until the reboot, no edit can land in
the old root. If the trial rolls back, the old root boots again unfrozen, and the refused edits were refused, not
lost.

### 3.4 What PR A does not add

- **Booting a named root**: present (section 1).
- **Clone, rename, delete and reaping**: present, and the crash corpus already exercises snapshots (format design
  section 7, milestone 9).
- **Cloning a root other than the booted one**: not needed. A deploy clones the running root.

## 4. The deploy

The deploy runs under the existing live-flash lock on the boot host and the operator's `deploy-lock.sh` target for
the R240. Live deliveries take the same lock, so a pet change does not race a deploy. The epoch check (4.6)
refuses a deploy when a change skipped the lock. On the box, each step is
`sudo -n <tool> paired ...` over the strict-host-key SSH session the live flash already uses, where `<tool>` is the
installer the host staged for this deploy (4.8), not the running generation's `/sbin/persistent-install`.

The host stages before any step runs on the box: first the tool (4.8), then the inventory, with `bput` (length
and SHA-256 checked, rename as the commit point) to `/root/.breenix-flash/<id>/inventory.tsv`. `paired begin`
reads the inventory for its free-space check, so the inventory must be present first.

### 4.0 How the installer writes a path

A path in the running root or the draft can hold a pet symlink (`/usr/lib` → `/home/...`, say). Following it
would send a root-owned write into `/home` or onto an existing file. So each write the paired installer makes, in
the running root (4.5), in shared `/opt` (4.5) and in the draft (4.3), resolves its path like this:
- open the tree's root directory with `O_DIRECTORY | O_NOFOLLOW`, and `fstat` it for its `st_dev`;
- walk each parent component with `openat(dir, name, O_DIRECTORY | O_NOFOLLOW)`. A symlink (`ELOOP`) or a
  non-directory (`ENOTDIR`) refuses the step. A component whose `st_dev` differs from the root's (a mount point,
  such as a bind to `/home`) also refuses it. A missing component is created with `mkdirat` at 0755 root:root and
  then opened the same way;
- create the leaf as a fresh temporary name in that directory with `O_CREAT | O_EXCL | O_NOFOLLOW`, write and
  `fsync` it, and `renameat` it over the final name in the same directory. A rename replaces a final symlink
  itself rather than its target. When the final name exists, its type must be the one the step expects (4.5 lists
  the cases), or the step refuses;
- `symlinkat`, `linkat`, `fchmodat` and `fchownat` (with `AT_SYMLINK_NOFOLLOW`) take the walked directory's fd.

The kernel already provides these calls and flags (`O_NOFOLLOW` in syscall/fs.rs:26-31; `Mkdirat`, `Symlinkat`,
`Linkat`, `Renameat`, `Fchmodat` and `Fchownat` in syscall/handler.rs:372-428). `local.rs`'s `install_at` and
`incoming` already refuse symlinked parents with `symlink_metadata`, but a check and a later path-based write leave
a window, so the paired installer uses descriptors instead.

### 4.1 Reclaim, then clone

1. `paired reclaim <retained...>`. The host passes the retained set (section 2). The box adds the booted
   generation itself, then deletes with `SNAPSHOT_DELETE`:
   - each `gen-<metal id>` root that is not retained;
   - each `<metal id>.new` draft.

   Each delete is its own committed transaction. The reaper frees the space in the background, 16 nodes per commit
   (audit, BreenFS locals), so freed space returns gradually. The free-space check in step 2 counts only free space,
   not DEAD space still being reaped.
2. `paired begin <id>`:
   - check statfs free blocks against twice the inventory's regular-file bytes plus the global reserve;
   - on the first paired deploy only, convert the toolchain selection (4.5);
   - read the epoch (E1);
   - write `/run/breenix/paired/<id>.epoch` (4.0's walk, mode 0600): E1 and the boot ID from
     `/run/breenix/boot-id`. The file is outside the draft, so apply's removal of `.breenix-incoming` and
     publish's unmount leave it in place. The epoch restarts at 0 on each mount (3.3), so publish refuses when the
     file's boot ID is not the running one, and a missing file refuses too;
   - `SNAPSHOT_CREATE` `<id>.new`, flags 0, on `/`;
   - mount it with `root-volume` at `/run/breenix/paired/stage` (3.2). `/run` is emptied at boot, so a crashed
     deploy leaves no stale mount point.

### 4.2 Upload, with checksums

1. The inventory is already staged (section 4's preamble).
2. `paired have <id>` prints the inventory rows the clone already satisfies byte for byte: same type, mode, owner,
   size and SHA-256 at the same path. Only the remaining rows need bytes from the host.
3. The host writes each remaining regular file once, as a content-addressed object, then runs
   `bsync --sudo objects/ host:/run/breenix/paired/stage/.breenix-incoming/objects/`. The objects go straight into the
   draft, so each byte crosses SSH once and is written once, with no second copy from a staging area.
   bsync's `bput --adopt` checks each object's length and SHA-256 before publishing it (tools/bsync/README.md).
   Each object is named by its own SHA-256, so the installer can check each object against its name. Objects are plain files at mode 0600. bsync refuses
   symlinks and set-ID bits, so the installer creates those from the inventory.

**#3226 is still open.** Writes are bounded: the installer runs `syncfs` after each 32 MiB it installs. The host
reports the size of the object set before uploading. If the set is larger than 1 GiB, it stops and asks the
coordinator. DEV-23's large-write ban covers `/home`. The payload goes to the system volume, which already receives
the live flash's kernel image (up to 512 MiB, `MAX_IMAGE` in live_flash.rs).

### 4.3 Apply

`paired apply <id>` writes the inventory into the draft. The draft is a clone of the running root, so pet changes
are already in it. The rules come from `LocalFiles` (local.rs), applied in place:

| Path in the draft | In the old baseline (`.breenix-image`) | In the inventory | Result |
|---|---|---|---|
| matches the inventory row | any | yes | kept as is |
| matches the old baseline | yes | yes, different | replaced from the object |
| differs from both | yes or no | yes | an on-box change. It is moved within the draft (one rename, no copy) to `.breenix-carried/<previous id>/<path>` with set-ID cleared, then replaced. It is listed in the receipt. |
| matches the old baseline | yes | no | removed (the image dropped it) |
| differs from the old baseline | yes | no | kept, and listed as an on-box path |
| absent from the baseline | no | no | kept (a pet addition) |

With no baseline (a generation that predates `.breenix-image`), a payload path that differs from the inventory
is treated as an on-box change: it is carried, not lost.

Construction uses the existing `Copier` rules:
- Directories are created first. A regular file is renamed in from its object (same tree), or linked if its
  object was already used by an earlier member of its link group.
- Symlinks are created with their target.
- Modes, owners and set-ID bits come from the row. `layout::check_setid` runs at the end.
- Directory mtimes are set last, bottom-up, so pet entries do not change them.

Finally the installer writes:
- `usr/share/factory/generation` = `<id>`;
- a new `.breenix-image` baseline from the inventory;
- `.complete` = `layout::record(id)`.

Then it removes `.breenix-incoming`. Apply is idempotent: a rerun after an interruption finds finished paths
already matching and continues.

### 4.4 Verify

`paired verify <id>` syncs the draft, then reads back each inventory path and encodes its rows exactly as the
publisher did. It prints `paired verify <id> rows=<n> sha256=<digest of the encoded rows>`. Any row that differs is
an error, and the deploy stops: `paired abort <id>` detaches the staging mount and deletes the draft (a crashed
deploy's draft is reclaimed by the next one). The host compares the digest with its own
inventory's. Extra on-box entries under payload directories are listed but do not count as differences.
This readback goes through the running kernel's caches. The trial boot runs the same verify again with cold
caches (4.7).

### 4.5 The toolchain selection

The toolchain version directories `/opt/breenix/rust/<toolchain id>/` stay in shared `/opt`. They are
content-addressed and immutable, several coexist, and they are installed as `scripts/devbox/README.md` describes.
Only the selection moves into the generation:

- The generation file `/usr/lib/breenix/rust-current` holds the selected toolchain ID. `layout.tsv` gains
  `dir usr/lib/breenix`. An E6 inventory carries the file as a payload row, and `/lib` carries the matching
  runtime.
- In shared `/opt`, `/opt/breenix/rust/CURRENT` becomes a symlink to `/usr/lib/breenix/rust-current`. Readers
  (`btest`, `bdev`, `bbench`, bcargo) keep reading `/opt/breenix/rust/CURRENT` and resolve the booted generation's
  selection. The devbox flow's `bput` of `CURRENT` and its separate `/lib` runtime sync are replaced by the
  paired deploy.
- **One-time conversion**, by the first paired deploy, inside `paired begin` before it reads E1 and clones (4.1):
  1. write `usr/lib/breenix/rust-current` into the running root with the ID that shared `CURRENT` names today
     (a pet change, and the only write a deploy makes to the running root), by the walk in 4.0 from `/`. The final
     name may be absent or a regular file. A symlink or a directory there, or at `usr`, `usr/lib` or
     `usr/lib/breenix`, refuses the conversion with the path named;
  2. rename a symlink over shared `CURRENT`, by the walk in 4.0 from `/opt`. `opt/breenix/rust` must be real
     directories on the default tree. The final name must be a regular file (the old selection) or a symlink
     whose target is already `/usr/lib/breenix/rust-current` (a repeated conversion).

  The rename is atomic. Before it, readers see the old file. After it, they see the same ID through the running
  root's new file, so the running system's toolchain does not change. The clone then inherits the file, so both
  possible rollback targets (the running root, and the new generation) have one. Doing this before E1 keeps the
  conversion's own write out of the epoch check. Apply replaces the draft's copy only when the inventory carries a
  selection row (E6). A legacy inventory keeps the running selection.
- Each version directory needs its own `<toolchain id>/MANIFEST.json`. `paired verify` checks that the version the
  draft selects is present and complete before the deploy can publish. A version that no retained generation
  selects stays where it is. Removing it is an explicit operator action, not part of reclamation.

### 4.6 Publish

`paired publish <id>`:
1. Read E1 and its boot ID from `/run/breenix/paired/<id>.epoch` (4.1), before anything else changes. A missing
   file, or a boot ID that is not the running one, refuses publish and leaves the draft for `paired abort`.
2. Unmount the staging mount (detach syncs the draft).
3. `ROOT_FREEZE` the running root and take E2 from it (3.3). If E2 differs from E1, thaw, delete the draft and
   exit with `running root changed during the deploy (epoch E1 -> E2); rerun`.
4. `SNAPSHOT_RENAME` `<id>.new` → `gen-<id>`, one committed transaction. A root named `gen-<id>` exists only after
   apply, verify and the toolchain check have passed.

The running root stays frozen through the kernel and manifest publication and the reboot (4.7). If the host
stops arming after the freeze (a failed `bput`, a lost connection), it runs `paired abort <id>`, which thaws.

### 4.7 The manifest, the manager and the kernel

The host builds the trial slot image with `slot_image(..., generation=<id>, writable_root=True)`. Its sealed
intent says `userland <id>`, the kernel mounts `gen-<id>`, and the image's SHA-256 binds the two. CURRENT and
PREVIOUS keep naming their own roots inside their own images. A slot rollback is therefore also a root rollback,
with no separate pointer to keep in step.

**Manifest version 2** adds one line after the six lines of version 1 (`live_flash.rs`). The line is the root
name, `gen-<id>`. Before writing the trial image, a manager that reads version 2:
- finds the ROOT item with that name in the space tree, using the default-tree reader `breenfs_files.rs` already
  opens, extended with a by-name scan of the ROOT key range (key `(0xffff_ffff_ffff_ff10, 0x90, *)`);
- requires flags 0;
- reads `\ramdisk` from the attached trial volume through the firmware's FAT driver, and requires its `userland`
  line to name the same `<id>`.

Any mismatch is `live flash refused` and the manager boots CURRENT. Version 1 manifests (kernel-only flashes) keep
today's behavior. The host writes version 2 only when the installed manager supports it (`disk.json` records the
manager commit that `--replace-manager` installed). Until then, the root is named only through the image's intent.
A missing root then costs one failed trial, ended by the 600 s deadline, instead of a refusal before the trial.

The host publishes `kernel.img` with `bput`, then the manifest, which arms the attempt. Then it reboots once
(#3247's sequence, metal_live_flash.py:236-243). The manager writes TRY.IMG, publishes LIVE.ID and boots the trial.
The trial kernel mounts `gen-<id>` read-write with the default tree, the shared binds and `/home`, exactly as a
persistent boot does today.

### 4.8 The installer that runs

Neither installer on the box can run the first paired deploy. Before PR B, the running generation's
`/sbin/persistent-install` has no `paired` command. The payload's own copy sits in the draft only after `begin`
and the upload, which need an installer to run. A payload built from a main that predates PR B also carries an
installer without `paired`, so the trial could not run the cold verify (section 5) with its own copy.

So each deploy brings its own tool, built by the host from the metal tools' commit (PR B's head for the live run)
as a static program for the box's architecture:
1. Before any step, the host publishes it with `bput` to `/root/.breenix-flash/tools/<sha256>/persistent-install`,
   mode 0700. `/root` is the default tree's directory, shared by both pairs (vfs/boot.rs:200), and the sudo
   policy's `permit %wheel as root nopass` (docs/users/design.md, sudo) lets the session run it as root.
2. The host runs `sudo -n <tool> paired protocol` and requires `BREENIX-PAIRED 1` before any other step.
3. The steps on the running system (section 4.1 to 4.6), and the trial's cold verify after the reboot, run that same file. It is
   still present on the trial, because `/root` is shared, and the host rechecks its SHA-256 there before using it.

The tool needs only the kernel's existing syscalls and the BreenFS ioctls of PR A, so it runs on the running and
the trial kernel alike. If it cannot run on the trial (an ABI the trial kernel refuses), the cold verify fails,
`committed` is not written, and the pair rolls back (section 5). The payload's own `/sbin/persistent-install` is
installed as payload, like any other file, and the deploy does not run it. A tool directory that no retained
deploy record names is removed by `paired reclaim`.

## 5. Try once, commit, roll back

**Commit.** After the reboot, the host's existing check (PID 1 is `/sbin/init`, a new boot ID,
metal_live_flash.py:246-263) gains three conditions:
- `/proc/mounts` shows `/` as source `gen-<id>`, `rw`;
- `/usr/share/factory/generation` reads `<id>`;
- `sudo -n <tool> paired verify <id> --booted`, run with the staged tool (4.8), returns the inventory digest. This is the cold readback after a reboot.

Only then does the host publish `committed` (the token) with `bput` to `/root/.breenix-flash`. That boot stays up
as the persistent session. The manager's next startup promotes the trial to CURRENT, and the old CURRENT, still
naming the old root, becomes PREVIOUS. `live-flash.json` gains `root`, `previousRoot`, `inventorySha256`,
`carried` (the on-box changes moved aside) and `onBox` (the on-box paths kept).

**Rollback.** If `committed` does not become durable (no SSH or a failed check within 600 s, a reset, a power cut,
or a loader that returns), the next boot finds an unacknowledged attempt. The manager boots CURRENT, which names the
old root (`Outcome::Rollback`, live_sequence.rs). The host records the rollback only after reading the old kernel
commit and `/` source `gen-<old>` over SSH. The failed generation stays as a reclaimable root.
`metal-run install-persistent --rollback` swaps CURRENT and PREVIOUS, both kernel and root, as it does today.

**What rollback restores, and what it does not.**
- Rollback restores the old kernel and the old generation as they were when the new pair was cloned.
- Changes made on the new pair stay in the new generation.
- Shared state is not generation content (design section 6, step 5), so it keeps whatever the trial session wrote:
  `/etc`, `/var`, `/opt`, `/srv`, `/root`, `/tmp`, `/run`, and `etc-reconcile`'s additions of missing defaults.
  Old credentials do not come back. Factory defaults therefore have to stay additive across a pair.
- `/home` likewise keeps any writes made during the trial session.
- BreenFS's copy-on-write commits and checksums protect both pairs against crashes and torn writes. They do not
  protect against a kernel that commits wrong metadata to the shared volume. A pair whose kernel changes the
  on-disk format cannot roll back, so paired deploys refuse a kernel that sets an incompat bit the running kernel
  lacks. Format v2 converts offline (DEV-29 Q3).

### 5.1 ESP slot state without renames (PR C)

**The defect, from the code.** Both slot actions (commit and rollback) and the attempt record move files by rename: `slot_transaction`'s
`move_pair` and `live_sequence::publish` (delete the old name, then rename the temporary one). `Root::write` also
deletes and recreates a file (files.rs:116). A FAT rename is a directory-entry edit that the firmware's driver may
write as more than one sector (the FAT specification's long-name entries sit beside the short entry), so a power
cut can leave both names, or neither. `move_pair` returns `invalid` for both cases, `recover` fails, and
`main.rs:317-320` returns to firmware before trying CURRENT or PREVIOUS. The host fixture
(`tests/live_sequence.rs:109`) moves a name in one step, so no test reaches the torn states. Deleting either of two
names that share one cluster chain would free clusters the other still uses, so recovery cannot repair a torn
rename by deleting a name.

**The design.** PR C changes the manager's ESP state so that no step depends on a rename:
- **Images are written once.** Each new slot image (a trial, or an image the installer writes) goes to a fresh
  file `K<seq>.IMG`, `<seq>` being seven decimal digits taken from the record sequence below, which only grows, so
  no name is reused. An image is written, then flushed, and is not renamed or rewritten afterwards.
- **The slots are a record, kept twice.** `\EFI\BREENIX\A\SLOTS.REC` and `\EFI\BREENIX\B\SLOTS.REC` are each one
  4096-byte file, allocated when the manager is installed and afterwards only rewritten in place at the same length
  (a new `Root::overwrite`: open without create, write at offset 0, flush). They sit in separate directories, so
  their directory entries are in different sectors. A record holds a magic line, a sequence number, the attempt
  manifest (up to `MANIFEST_MAX` bytes, section 4.7), and for CURRENT, PREVIOUS and a trial each: the image name,
  size, SHA-256 and note bytes. A SHA-256 over the rest ends it. A record whose digest does not match is ignored.
- **Each of the four transitions is one record write.** Try, promote, commit and rollback each build the next state, write it
  into the record that does not hold the newest valid sequence (with sequence + 1), and flush. A cut leaves the
  old record valid, or the new one valid, or the new one torn and ignored. The attempt record (`LIVE.ID` today)
  is the record's attempt field, so it changes in the same write.
- **Old images are deleted last.** After a record write is durable, the manager deletes each `K*.IMG` that neither
  valid record names. A cut there leaves a stray image, which the next startup deletes.
- **Startup returns to firmware only after the named images fail.** The manager reads both records and uses the
  newest valid one. It boots that record's CURRENT, and if CURRENT is missing, has the wrong size or its bootloader
  returns, it checks the image's SHA-256 against PREVIOUS's entry and boots PREVIOUS, then the older record's two
  images. It returns to firmware only when no named image verifies.
- **Import from today's layout.** A manager that finds no valid record runs today's `slot_transaction::recover`.
  If that succeeds, it writes record A naming the existing `CURRENT.IMG` and `PREVIOUS.IMG` in place (the record
  holds names, so no file is renamed). If it fails, the manager does not return to firmware. It boots, with no ESP
  write, the first of `CURRENT.IMG`, `PREVIOUS.IMG`, `NEXT.IMG`, `SWAP.IMG` and `TRY.IMG` that passes
  `bootdisk::looks_like_disk`, and prints `slot recovery failed; booting <name> read-only`. That box then needs the
  netbooted recovery route to repair its ESP.

PR C also ships manifest version 2 (section 4.7), because both reach the box only through a manager replacement.

| Test | Where | Checks | Red on the old code because |
|---|---|---|---|
| T-C1 power cut at each ESP write | `scripts/metal/efi-stub/tests/live_sequence.rs`, with a fixture whose overwrite can stop after any 512-byte sector and whose rename leaves both names or neither | For try, promote, commit, rollback and the import, a cut after each device write. The next startup boots a pair whose image verifies, and the pair is the old one or the new one. | The fixture's torn rename makes `recover` fail, and startup returns to firmware |
| T-C2 failed import | same file | Both names, or neither, for CURRENT→PREVIOUS: startup boots a verified image without writing the ESP. | `main.rs:317-320` returns to firmware |

## 6. CURRENT, PREVIOUS and reclamation

At rest, the system volume holds the default tree, `gen-<current>` and `gen-<previous>`. While a committed trial
runs unpromoted, it also holds the trial's root. During a deploy, it also holds one draft or one published but
unselected `gen-<id>`. Retention is the retained set from section 2, so a root leaves it only after the manager has
reported a promotion that drops it from both physical slots.
Reclamation runs at the start of each deploy (4.1), and as `paired reclaim` on its own:
- it deletes the metal generations and drafts outside the retained pair;
- it does not delete the booted root (the kernel refuses with `EBUSY`), root 1, an attached staging root, or a root
  whose name does not match the metal-ID pattern. That last group includes the migration's `gen-<ext2 id>` until a
  deploy retires it from the slots.

Clones share unchanged nodes and extents. A generation's own cost is its changed files plus their
copy-on-write paths.

## 7. /home

`/home` is the separate BreenFS home partition, mounted read-write `nosuid,nodev` by each persistent boot. A
deploy does not open it, write it or snapshot it. The netbooted BreenFS upgrade took a `pre-upgrade-<id>`
read-only home snapshot (breenfs.rs:116). The paired deploy does not, because DEV-40 leaves `/home` untouched and a
snapshot would pin home's freed space for as long as the pair exists. The trial and rollback kernels mount the
same `/home`.

## 8. Power-loss points

| # | Lost during | State after recovery | Next boot | Next deploy |
|---|---|---|---|---|
| 1 | Reclaim (4.1) | Each delete is committed or not. The reaper resumes. | CURRENT pair | Repeats the deletes still needed |
| 2 | Toolchain conversion (4.5), first deploy only, before the clone | Before the rename: shared `CURRENT` is the old file (the running root may already hold its own selection file with the same ID, which no reader uses until the symlink exists). After: the symlink. | CURRENT pair, with the same toolchain either way | Repeats the conversion if the symlink is missing |
| 3 | `SNAPSHOT_CREATE` of the draft | No draft, or a complete clone | CURRENT pair | Deletes the draft |
| 4 | Upload or apply into the draft | A partial `<id>.new`, which no slot names. The running root and shared state are as committed. | CURRENT pair | Deletes the draft and starts again |
| 5 | Publish rename | `<id>.new` or `gen-<id>` | CURRENT pair (no attempt is armed) | Reclaims it if it is no longer wanted |
| 6 | `kernel.img` or manifest publication | The old manifest or no manifest. `bput`'s rename is the commit point, and the manifest is written after the image. | CURRENT pair, with no attempt | Arms again |
| 7 | Manager writing the trial image and the record that names it (5.1) | Before the record write is durable, the older record is newest and no attempt is recorded. After, the attempt is used. A torn record is ignored. | Before: the manager retries the trial. After: rollback to the CURRENT pair. | n/a |
| 8 | Trial session, before `committed` is durable | The new generation holds whatever the trial wrote | Rollback to the CURRENT pair | Reclaims the failed generation |
| 9 | `committed` publication | Present or absent, by rename | Present: the manager promotes the pair. Absent: rollback. | n/a |
| 10 | Manager promotion | With PR C's manager: one record write, so the old or the promoted record is newest. With today's manager: `SLOT.TXN` resumes the renames, but a torn rename fails recovery and returns to firmware (5.1). | PR C: the old or the promoted pair. Today: the netbooted recovery route. | n/a |
| 11 | Any BreenFS commit on the way | Commit G is either durable or absent, and G−1 is intact (format 2.5) | As above | As above |

With PR C's manager, each row above leaves the box booting a complete pair whose kernel and root were committed
together. Only rows 7 to 10 can change which pair that is, and the manager's record and the `committed` receipt
decide it. With today's manager, rows 7 and 10 can also leave a torn rename, which needs the recovery route.

## 9. Implementation plan: three PRs

PR A (kernel) and PR C (boot manager, section 5.1) are independent. PR B (tools and installer) uses both.

Both PRs follow DEV-30 and DEV-39. Each test is committed first and fails on the old code. Tests are deterministic
and run once, at the final head. No suite runs. No new warnings on x86-64 or ARM64. Each PR gets the
other-family review.

### PR A: BreenFS and kernel

Scope: sections 3.1 to 3.3.
- `kernel/src/fs/breenfs/ops.rs`: the staging tree.
- `vfs.rs`: attach, detach and the epoch.
- `abi.rs`: `ROOT_EPOCH`, `ROOT_FREEZE` and `ROOT_THAW`.
- `mount.rs`: the `root-volume` source and unmount detach.
- `docs/fs/breenfs-design.md` 2.6: one paragraph naming the interface.

There is no on-disk format change. The check commits add stubs with the final signatures that return today's
behavior: `EBUSY`, a second writer refused. The tests then fail at run time.

| Test | Where | Checks | Red on the old code because |
|---|---|---|---|
| T-A1 staging write isolation | `tools/breenfs-host-tests/tests/snapshot.rs` | A persistent writer (`gen-A` plus the default tree) clones `gen-A` to `B.new` and attaches it. It then creates, overwrites, truncates, unlinks, hard-links, symlinks and chmods in the staging tree, interleaved with booted-root and default-tree writes, across several commits. After a fresh recovery mount, `B.new`'s migration manifest equals the model, and `gen-A` and the default tree equal their own models. | Attach returns `EBUSY` |
| T-A2 crash images | `tools/breenfs-host-tests/tests/crash_images.rs` | The same stream, through detach and the rename to `gen-B`, under the recording device. Each crash image recovers and passes `breenfs-check --full`. `gen-A` and the default tree match the model at the recovered commit. The second root is absent, a `B.new` that equals some prefix of the staged stream, or a complete `gen-B`. | The stream stops at attach |
| T-A3 refusals and epoch | `tools/breenfs-host-tests/tests/snapshot.rs` | Attach is refused for the booted root, root 1, a read-only root and a second staging tree. Delete and rename of the attached root return `EBUSY`. Detach with an open staged file returns `EBUSY`. The epoch does not change for reads (atime), default-tree writes or staging writes, and does change for a booted-root write. After `ROOT_FREEZE`, a booted-root create, write, rename and setattr return `EROFS` and the epoch stays at the frozen value, while default-tree writes and atime touches still succeed. `ROOT_THAW` admits writes again. A booted-root write racing the freeze, driven by a fixed interleaving, is either counted in the returned epoch or refused. | No attach, no epoch and no freeze |
| T-A4 unmount keeps the writer | `tools/vfs-host-tests/tests/boot_mount_tests.rs`, which already mounts and unmounts through the registry | Unmounting the staging mount detaches it, and `/` stays registered, writable and committing. Order: with a process's working directory inside the staging mount, unmount returns `EBUSY` and the mount is still registered and still attached (a staged write still succeeds); after that directory is left, unmount succeeds with no reference from its own walk blocking detach. Access: a non-root credential's lookup of `/run/breenix/paired/stage/usr` returns `EACCES`. | Today's non-bind unmount path calls `shutdown` |

No boot belongs to PR A. Its live effect is the staging mount PR B uses on the R240. The kernel reaches the box
through a kernel-only one-reboot flash (#3247) of a main that contains PR A, before PR B's live run (below).

### PR B: metal tools, installer, boot manager and the live run

Scope: sections 4 to 6.
- `userspace/programs/src/persistent_install/paired.rs`: subcommands `protocol`, `reclaim`, `begin`, `have`,
  `apply`, `verify`, `publish` and `abort`, reusing `Copier`, `local`, `layout::record` and `check_setid`.
- `libs/libbreenix/src/layout.tsv`: `dir usr/lib/breenix`.
- `scripts/metal/metal_live_flash.py` and `metal-run`: `install-persistent --paired [--sha|--branch]
  [--inject paired-no-commit]`. This covers the inventory encoder for a legacy userland tree, the object set, the
  steps, the commit check, the receipts and the retained set.
- `scripts/metal/efi-stub/src/live_flash.rs`, `live_sequence.rs`, `breenfs_files.rs` and `manager/live.rs`:
  manifest version 2 and its checks.
- `scripts/devbox/README.md`: the toolchain selection.
- `docs/metal/README.md`, "Kernel flash and /usr replacement": the paired deploy replaces the rollback paragraph's
  "both normal slots point to the same root".

| Test | Where | Checks | Red on the old code because |
|---|---|---|---|
| T-B1 apply over a fixture clone | `tests/persistent_install/` host test, beside `tests/metal_replace_usr_host.rs` | A fixture clone with a baseline, an inventory and objects. After apply and verify: the payload rows equal the inventory; a pet file at an unmanaged path is kept; a locally changed managed file is moved to `.breenix-carried/<prev>/` with set-ID cleared; a dropped, unchanged managed file is removed; a BusyBox hard-link group across `/bin` and `/sbin`, a symlink and set-ID `su` are exact; mount points are untouched; a rerun after a cut at each file is idempotent. Path safety (4.0): with a pet symlink at `usr/lib` pointing outside the tree, at `usr/lib/breenix/rust-current`, and at `opt/breenix/rust`, the conversion and apply refuse, and the symlinks' targets (a fixture file standing in for one in `/home`) are byte-for-byte unchanged. Epoch custody: begin, apply and publish run in sequence, and publish compares the E1 that begin recorded after apply has removed `.breenix-incoming` and the unmount has run; a recorded boot ID that differs from the fixture's refuses publish. | The check commit's stub routes `apply` to today's only in-place path, `replace_usr`, which leaves `/bin`, `/sbin` and `/lib` unchanged |
| T-B2 manager version 2 | `scripts/metal/efi-stub/tests/live_sequence.rs` | The trial is refused for a missing root, a read-only root, or an image intent naming another userland. It is accepted when the three agree. Version 1 behaves exactly as today. | `Manifest::parse` rejects a seventh line |
| T-B3 host flow | `tests/metal_live_flash_test.py` | `prepare --paired`: the slot intent names the new generation; the manifest carries the root line only for a version-2 manager. The retained set keeps the physical PREVIOUS's root when an acknowledged trial is not promoted (a stale base or a foreign NEXT), and drops a root only after a `promoted` line removes it from both physical slots. The commit check needs `/` to be `gen-<new>` `rw` plus the verify digest. A rollback receipt needs the old commit and `gen-<old>`. Staging order: the tool and the inventory are published, and `paired protocol` answers, before `paired begin`; the eight box subcommands and the cold verify invoke the staged tool path, not `/sbin/persistent-install`. | Today's `prepare` names CURRENT's userland (metal_live_flash.py:100) |

**The live run on the R240** is the only box activity. It goes through the one-reboot path, under the
persistent lease, with no netboot and no suites. Steps:

0. **Prerequisite.** The running kernel contains PR A. If the coordinator's batched kernel-only flash has not
   already carried it, PR B's brief names one kernel-only live flash of main.
1. **One paired deploy of main.** The tools come from PR B's head. The kernel and the userland inventory are built
   from `origin/main`. The tool that runs the steps and the trial's cold verify is PR B's build, staged as in 4.8,
   because main's installer, which the payload also carries, predates the
   protocol. The pair commits. Evidence:
   - `live-flash.json` (`result=committed`, `root=gen-<id1>`, `previousRoot`, `inventorySha256`, `carried`, `onBox`);
   - the serial's `live flash: trial` line;
   - `/proc/mounts` and the cold verify digest.
2. **One forced rollback.** A second paired deploy of the same build with `--inject paired-no-commit`. The trial
   boots `gen-<id2>`. The host reads SSH on it, writes a marker file into `gen-<id2>`'s `/usr/share`, withholds
   `committed` and reboots. The manager boots the step-1 pair. Evidence:
   - `result=rolled-back`;
   - the manager's `live flash: rollback <token>` and `booting current <commit>` serial lines;
   - `/` = `gen-<id1>`. Both steps build the same kernel commit, so the root identifies the slot: `gen-<id1>` is
     named only by the step-1 image's sealed intent, and `gen-<id2>` only by the trial's;
   - the marker absent.

   `upgrade-no-ssh`'s invalid loader stops before the kernel mounts the new root, so it cannot show the root
   returning. This inject can.
3. **Reclaim.** `paired reclaim` deletes `gen-<id2>`. A `SNAPSHOT_NEXT` listing shows `gen-<id1>`, the generation
   that was running before step 1 (physical PREVIOUS once step 2's reboot has promoted step 1) and the default tree.
4. **End state.** The box is left on the main pair (step 1), answering SSH. PID 1 is `/sbin/init`.

That is three reboots in total (one in step 1, two in step 2), plus the step-0 flash if it is needed.

## 10. Deviations from the ruling's wording (DEV-39(2))

- **"A writable clone of a snapshot of the running root"** is one `SNAPSHOT_CREATE` with flags 0. That call shares
  the running root's committed tree in a new writable ROOT. An intermediate read-only snapshot would add no
  protection beyond what the old generation, kept unmounted as PREVIOUS, and the epoch check already give.
- **"The boot manager passes the root generation to the kernel"** is done by attaching the slot image whose sealed
  intent names it. The manifest's new root line is checked against that intent, not written into it, which keeps
  the single sealed pairing the installer relies on (persistent_install.rs:1-3).
- **`/opt/breenix/rust/CURRENT`**: only the selection moves into the generation, and the version directories stay
  in shared `/opt`. That gives an exact rollback without copying toolchains between trees: `Files` refuses an
  edit whose inodes span two trees (ops.rs:855), so a toolchain cannot be renamed from `/opt` into a generation.
- **The live run does not replace the manager.** Version 2's checks are covered by T-B2, and the live run uses the
  installed manager's version. Replacing the manager needs a recovery netboot (README "Kernel flash"), which
  DEV-40(3c) does not name.
- **The live run's forced rollback uses a new inject, `paired-no-commit`**, instead of `upgrade-no-ssh`. A broken
  loader stops before the new root is mounted.
- **No home snapshot**, unlike the netbooted BreenFS upgrade, because DEV-40 leaves `/home` untouched.
- **The README section the brief calls 'Flash and rollback contract'** is "Kernel flash and /usr replacement" in
  `docs/metal/README.md`. Its rollback paragraph is the one #3263 links.

## 11. For the coordinator

- PR B's step 0 needs the box's kernel to contain PR A. Either the batched flash carries it, or PR B's brief names
  that flash.
- #3226 is open. The upload is bounded (4.2), and the host stops before uploading more than 1 GiB.
- The manager's version-2 check reaches the box at the next manager replacement, which the coordinator schedules.
  Until then, a missing root costs one failed trial and the 600 s deadline.
- **Decision needed: PR C changes the ESP's slot layout** (an on-disk format of the boot manager). It replaces
  today's renamed `CURRENT/PREVIOUS/NEXT/TRY` files and `LIVE.ID` with write-once images and two in-place
  records, and imports today's layout on first start. Today's manager, which the live kernel flash already uses,
  returns to firmware after a torn rename. PR C reaches the box only through a manager replacement by the netbooted
  installer, as DEV-33(3) authorized once before. The recommendation is to schedule that replacement before PR B's
  live run, so the live run exercises PR C's records and the version-2 manifest.
