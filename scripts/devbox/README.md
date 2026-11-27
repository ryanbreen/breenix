# Stage the hosted Rust toolchain

Run the stager on the Mac with Python 3.11 or later and the Rust host toolchain's
`llvm-tools` component. It consumes retained E25 artifacts. The K5, K6, K7 and
hosted libc pins are under `toolchain/rust-host/`.

```sh
out="$HOME/Library/Caches/breenix-devbox/toolchain"
scripts/devbox/toolchain.py stage --arch x86_64 --out "$out"
```

The command prints the version directory name. Use `--artifacts`, `--sysroot`,
`--fixture`, `--cargo`, `--bld` and `--kd` to point at retained artifacts when
they are outside the default build directories. The `--bld` directory includes
the E25 build receipt with `files_sha256.bld`. The `--kd` directory includes the
two runtime binaries and the KD `build.json` with their SHA-256 digests. For
artifacts recovered from an E25 self-host companion image, take these fields
from `toolchain.tools.bld.sha256` and `stagedFiles["/lib/<name>"]` in that image's
build receipt.

The version contains `bin/`, `lib/`, both target specs and the x86_64-breenix
sysroot. Its `MANIFEST.json` records the fork commit and each payload's size,
mode and SHA-256. The version's manifest hash covers that JSON before adding
the `id` field. The staged runtime is at `"${out%/*}/runtime/x86_64"`. Use a
fresh staging directory when the local runtime bytes need to change.

Create `/opt/breenix/rust` with `sudo -n mkdir -p -m 0755` on the destination
before using `bsync --sudo`. Sync the version directory to
`/opt/breenix/rust/<id>` and the runtime directory to `/lib`. Check the
installed bytes against the Mac's manifest and confirm root ownership and
modes. Run the pinned `rustc -vV` and `cargo -V` from the candidate directory.
Pass `RUST_TARGET_PATH=<version>/targets` to rustc.

Once the candidate passes, generate the one-line activation payload with
`scripts/devbox/toolchain.py current --out "$out" "$id"`. Publish it atomically
with `sudo -n bput` to `/opt/breenix/rust/CURRENT`, supplying its length,
SHA-256 and mode `0644`. Use the operator's login entry in `/etc/bshrc` to
read `CURRENT` and add the selected version's `bin/` to `PATH`. The login
entry also sets `RUST_TARGET_PATH` to the version's `targets/`.

Run the Mac checks with
`python3 -W error -m unittest tests.devbox_toolchain_test`.
