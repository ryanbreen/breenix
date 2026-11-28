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

Create `/opt/breenix/rust` with `sudo -n mkdir -p -m 0755` before using
`bsync --sudo`. Set `host` to your SSH destination and `id` to the staged
version. Sync `"$out/$id"` to `"$host:/opt/breenix/rust/$id"`. Check the
installed files against the Mac's manifest, including ownership and modes.

Publish the selection as `/usr/lib/breenix/rust-current` in the paired
userspace inventory. Include the version's matching runtime from
`"${out%/*}/runtime/$id"` under `/lib`. The paired installer validates the
version's manifest and the generation's runtime before publishing and again
after the trial boots.

On the first paired deploy the installer copies the current selection into
the running and retained rollback roots, then makes
`/opt/breenix/rust/CURRENT` a symlink to `/usr/lib/breenix/rust-current`.
The legacy inventory keeps the inherited selection. A paired inventory
can name a different version.

## Upgrade and roll back

Keep the installed version directories for the current and previous pairs.
Stage new version directories under their own IDs and use the
[paired metal deploy](../../docs/metal/paired-generations.md) to switch the
selection and runtime together. The previous pair restores its selection
and runtime together on rollback.

## Use the selected toolchain

Until the login PATH entry lands, read the version ID from
`/opt/breenix/rust/CURRENT` and invoke the tools by their full paths, such as
`/opt/breenix/rust/<id>/bin/rustc` and `/opt/breenix/rust/<id>/bin/cargo`.
When needed, set `RUST_TARGET_PATH` to `/opt/breenix/rust/<id>/targets` in
the build environment. The login PATH entry is tracked in #3204.

Run the Mac checks with
`python3 -W error -m unittest tests.devbox_toolchain_test`.
