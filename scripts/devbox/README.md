# Stage the hosted Rust toolchain

Run the stager on the Mac with Python 3.11 or later and the Rust host toolchain's
`llvm-tools` component. It consumes retained E25 artifacts. The K5, K6, K7 and
hosted libc pins are under `toolchain/rust-host/`. The same directory holds the
`devbox-x86_64.json` pins for the linker and loader from the E25 self-host
companion. Every staged file must match a repository pin.

```sh
out="$HOME/Library/Caches/breenix-devbox/toolchain"
scripts/devbox/toolchain.py stage --arch x86_64 --out "$out"
```

The command prints the version directory name. Use `--artifacts`, `--sysroot`,
`--fixture`, `--cargo`, `--bld` and `--kd` to point at retained artifacts when
they are outside the default build directories. The `--bld` directory contains
`bld`, and the `--kd` directory contains `ld-breenix.so` and `libc.so`.

The version contains `bin/`, `lib/`, both target specs and the x86_64-breenix
sysroot. Its `MANIFEST.json` records the fork commit and each payload's size,
mode and SHA-256. The version's manifest hash covers that JSON before adding
the `id` field. Each version keeps a separate runtime copy in
`"${out%/*}/runtime/$id"`. For an existing installation staged with the old
architecture-named runtime directory, stage its pinned artifacts under a fresh
output parent to create the versioned copy without modifying the box.

## Install and activate

Create `/opt/breenix/rust` with `sudo -n mkdir -p -m 0755` on the destination
before using `bsync --sudo`. Set `host` to your SSH destination and `id` to the
staged version. Sync `"$out/$id"` to `"$host:/opt/breenix/rust/$id"` and
`"${out%/*}/runtime/$id"` to `"$host:/lib"` with `bsync --sudo`.

Check the installed bytes against the Mac's manifest and confirm root ownership
and modes. Generate the activation payload on the Mac using fresh SSH output.
The command checks both installed runtime digests and the retained Mac copy for
the selected version.

```sh
ssh "$host" /bin/sha256sum /lib/ld-breenix.so /lib/libc.so > "$digests" &&
  scripts/devbox/toolchain.py current --out "$out" "$id" < "$digests" > "$payload"
```

Set `digests` and `payload` to files in the local deployment log directory.
Continue only if both commands succeed. Run the pinned `rustc -vV` and `cargo -V`
from the candidate directory with `RUST_TARGET_PATH=<version>/targets` for
rustc. Then publish the one-line payload atomically with
`sudo -n bput /opt/breenix/rust/CURRENT <length> <sha256> 0644` and read back
`CURRENT`.

## Upgrade and roll back

Before an upgrade, save the output of
`ssh "$host" /bin/cat /opt/breenix/rust/CURRENT` in the deployment log as the
previous ID. Keep its installed version directory, Mac manifest and retained
runtime. Run the activation payload check for that ID before any runtime
writes so a missing or damaged rollback copy stops the upgrade.

Serialize deployments. If either runtime digest changes, finish any programs
using this runtime and keep new invocations paused until the switch or rollback
completes. Both versions resolve the same absolute paths in `/lib`. The two
files are published individually, so a runtime change and the `CURRENT` update
are separate operations.

To roll back, set `id` to the saved previous ID. If the installed runtime
differs, sync `"${out%/*}/runtime/$id"` to `"$host:/lib"` with `bsync --sudo`
and verify both files' ownership and modes. Follow the same activation check,
version commands and `CURRENT` publication above. If a runtime transfer is
interrupted, complete it or restore the previous pair before resuming use.

## Use the selected toolchain

Until the login PATH entry lands, read the version ID from
`/opt/breenix/rust/CURRENT` and invoke the tools by their full paths, such as
`/opt/breenix/rust/<id>/bin/rustc` and `/opt/breenix/rust/<id>/bin/cargo`.
When needed, set `RUST_TARGET_PATH` to `/opt/breenix/rust/<id>/targets` in
the build environment. The login PATH entry is tracked in #3204.

Run the Mac checks with
`python3 -W error -m unittest tests.devbox_toolchain_test`.
