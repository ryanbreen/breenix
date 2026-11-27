# bsync

`bsync` pushes a local directory tree from macOS to a running Breenix system
using the Mac's `/usr/bin/ssh`, including its host aliases, keys and host-key
checks.

```sh
cargo install --locked --path tools/bsync

bsync --dry-run --exclude .git --exclude target ./source USER@HOST:work/source
bsync --exclude .git --exclude target ./source USER@HOST:work/source
bsync --sudo ./dist USER@HOST:/usr/local/bin
bsync --dry-run --delete ./dist USER@HOST:/srv/application
bsync --delete ./dist USER@HOST:/srv/application
```

The contents of `SRC` go directly under `DEST`. Relative destinations start
in the SSH login directory. The immediate parent of `DEST` must exist.
`bsync` creates missing directories within the destination and preserves the
source's ordinary Unix permission bits on files and directories. Paths must
be UTF-8 and contain no control characters.

Each run opens one SSH connection (an OpenSSH ControlMaster in a private
directory under `/tmp`) and runs its listing and each plan step as a channel
on it. It inventories the destination in one SSH exec using Breenix's
`sha256sum`, then prints the full plan before making changes. An unchanged
file is skipped. A mode-only file change sends no file bytes over SSH.
Ordinary uploads stage bytes through the SFTP v3 worker in `bsshd`, with up
to 16 writes of 32 KiB in flight, and `bput --adopt` publishes the staged
file itself, so its bytes are written once. With `--sudo`, `sudo -n` runs
the guarded remote commands and `bput` receives the upload on stdin. In
either case `bput` verifies the length and SHA-256 before publishing the
file by rename. Uploads need a `bput` that accepts `--adopt`.

Repeat `--exclude` to skip multiple patterns. A pattern without a slash
matches names at any depth. A pattern with a slash matches a path relative
to the tree root, with an optional leading slash. Trailing slashes are
ignored, so `build/` matches the same paths as `build`. `*` and `?` stay within
a path component and `**` as a full component spans directories. An
excluded directory and its descendants are also protected from deletion.
The destination walk skips excluded entries before reading their contents.
`--delete` removes destination-only entries and allows file/directory
replacement. Deletion uses individual file removal and empty-directory
removal.

The tool rejects traversal components and refuses to write through symlinks.
It checks the physical destination directory again when it applies each plan
step. Use a destination whose directories will stay in place during the
sync. Special files, symlinks in the source, and local set-ID or sticky
bits are unsupported. A remote symlink or special file that would be
replaced or deleted causes the plan to fail.

Run its host checks with `cargo test --locked --manifest-path tools/bsync/Cargo.toml`.
