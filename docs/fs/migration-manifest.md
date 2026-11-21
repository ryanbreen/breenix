# Migration manifest

The persistent migration and its independent image readers use the same
manifest to compare logical trees. See [the migration design](breenfs-design.md#6-migrating-the-persistent-install-s4).

The manifest is ASCII TSV with no header. Each row ends with LF. Emit a row
for the selected directory itself and for each name reachable beneath it,
without following symlinks. Order the rows by the unsigned bytes of their
absolute paths within that tree. The selected directory's path is `/`.

| Column | Encoding |
| --- | --- |
| Path | Lowercase hexadecimal of the raw path bytes, including the leading slash. |
| Type | `reg`, `dir`, `symlink`, `fifo` or `sock`. Devices use `chr:MAJOR:MINOR` or `blk:MAJOR:MINOR` with decimal device numbers. |
| Mode | Record `0777` for symlinks. For other types, record four octal digits for the permission, setuid, setgid and sticky bits. File type comes from the type column. |
| UID | Unsigned decimal owner ID. |
| GID | Unsigned decimal group ID. |
| Link group | For a non-directory inode with several names in the selected tree, repeat the encoded path of its first row for each member. Other rows carry `-`. |
| Size | Decimal logical byte length for a regular file or symlink target. Directories and special files use `0`. |
| Mtime | Signed decimal Unix seconds followed by a period and nine digits for the nanoseconds field. Use `000000000` for a seconds-only timestamp. Seconds and nanoseconds are the two timespec fields, including for times before the epoch. |
| SHA-256 | Lowercase hexadecimal of the digest of the regular file's logical bytes or the symlink target bytes. Other rows carry `-`. |

A zero-filled extent and a hole of the same logical length contribute the same
bytes to the digest. Directory allocation and file block counts are outside the
manifest. Link groups are local to the selected tree, so links outside it have
no effect on the group.

Compare the encoded manifests byte for byte.

```text
breenfs-extract IMAGE migration-manifest [--root NAME]
python3 scripts/metal/migrate_manifest.py ext2 IMAGE [--subtree PATH]
```

BreenFS defaults to its default root. A specified name selects the corresponding
root of the recovered image, including a read-only snapshot. Ext2 defaults to
its image root. `--subtree` selects an absolute directory path within the
image, without following symlinks in that path. Both commands read the image
without writing to it and report read errors with a nonzero exit status.

The guest migrator writes `/run/persistent-migrate/source.tsv` and
`destination.tsv`. Its selected tree joins the system default root's
`/.breenix-shared` and `/root`, the active generation at `/gen-<id>`, and
the home partition at `/home`. The generation's empty mount points remain
inside its own subtree. On the destination, the whole default root is
compared, so an unexpected entry also prevents a match.
On a match, the migrator prints `[migrate] normalized symlink modes: N` beside
the equality receipt. The count is the number of source symlink rows whose
stored mode bits differed from `0777`.

Run `python3 tests/persistent_migrate_test.py` with `TMPDIR` on a filesystem
that retains sparse extents when files close.
