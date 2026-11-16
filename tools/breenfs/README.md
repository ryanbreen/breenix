# breenfs host tools

Build with `cargo build --manifest-path tools/breenfs/Cargo.toml --bins`.

`breenfs-mkfs --size BYTES [--populate DIR] IMAGE` formats an image through the
shared transaction writer. `--uuid HEX`, `--hash-key HEX` and `--time SECS` fix
the identity, hash key and timestamps for reproducible images. Population accepts
directories, regular files and symlinks, records owners as 0:0, and retains hard
links within the source tree. It imports the data ranges reported by
SEEK_DATA/SEEK_HOLE, rounded outward to 4096-byte blocks. Trailing holes retain
the source size. Allocated data filled with NUL bytes remains mapped; bytes alone do not determine
whether a block is a hole. Hosts/filesystems that cannot report data ranges are
refused rather than silently changing the allocation policy.

`breenfs-check --full IMAGE` is the independent format checker.

## Reading an image

```sh
breenfs-extract IMAGE ls /
breenfs-extract IMAGE cat /nested/child.txt
breenfs-extract IMAGE dump
breenfs-extract IMAGE manifest
breenfs-extract manifest --dir DIR
```

Image commands use read-only recovery and the shared core reader. No recovery
cleanup is requested. `ls` prints immediate children sorted by name bytes, or
the named non-directory, as `type<TAB>hex-path`. `cat` writes regular-file bytes
to stdout, resolving symlinks with a bounded traversal. Paths start at root 1
(`default` in a new image); relative paths also start there. `ls` leaves the
final symlink unresolved. Names, symlink targets and file contents are read
without changing image metadata.

`dump` prints a recovered-superblock summary (generation, UUID, total blocks and
free blocks), then SPACE items in key order, then each ROOT's FS items in key
order. An item line gives its tree/root, decimal object and offset, hexadecimal
type, and complete encoded payload in lowercase hex. This is a logical item dump,
not a raw listing of unused sectors or abandoned generations.

## Manifest

The output is headerless TSV: one line per path, including `/`, sorted by raw
absolute path bytes. Hex encoding keeps tabs, newlines and non-ASCII bytes in
names unambiguous. The columns, in order, are:

| Column | Representation |
|---|---|
| path | Lowercase hex of the absolute path bytes, without a terminating NUL |
| type | `reg`, `dir`, `symlink`, `chr`, `blk`, `fifo` or `sock` |
| mode | Full `st_mode`, including type and special bits, in octal (at least six digits) |
| uid | Decimal owner user ID |
| gid | Decimal owner group ID |
| size | Decimal bytes for regular files and symlink targets; entry count for directories; 0 for special files |
| blocks | Mapped 512-byte units: eight per mapped breenfs block; no metadata charge; 0 for inline symlinks and directories |
| mtime seconds | Signed decimal seconds since the Unix epoch |
| mtime nanos | Decimal nanoseconds within the second |
| SHA-256 | Lowercase FIPS 180-4 digest of regular-file bytes (including hole zeros) or symlink target bytes; `-` for other types |
| link group | Hex path of the bytewise-first member of a non-directory link group with multiple paths in this tree; `-` otherwise |

`manifest --dir DIR` describes what population records with source timestamps
(without `mkfs --time`): owner 0:0, source modes and mtime, directory entry counts,
inline symlinks, source hard-link groups, and block-rounded data ranges using the
same SEEK_DATA/SEEK_HOLE policy as the formatter. It hashes regular files through
EOF. Links outside DIR do not form a group in the manifest. Unsupported source
kinds are refused as they are by population. Source trees must remain unchanged
while formatting or generating their manifest.

For example, a directory containing an `abc` file could have the following rows
(shown as a table for readability; the command emits tabs):

| path | type | mode | uid | gid | size | blocks | mtime seconds | mtime nanos | SHA-256 | link group |
|---|---|---|---|---|---|---|---|---|---|---|
| 2f | dir | 040755 | 0 | 0 | 1 | 0 | 123456789 | 0 | - | - |
| 2f66696c65 | reg | 100644 | 0 | 0 | 3 | 8 | 123456789 | 0 | ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad | - |

The host extraction check is `cargo test --manifest-path tools/breenfs/Cargo.toml --test extract`.
Kernel mount plumbing, guest fixtures and read-milestone runtime acceptance on
ARM64 remain subsequent work in step 24.10.
