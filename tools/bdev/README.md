# On-box builds

Install the Mac driver with `cargo install --locked --path tools/bdev`.
From a Breenix checkout, configure the SSH login, the absolute source
directory on the box and a separate cache directory on the box.

```sh
bdev --host "$BOX_LOGIN" --source "$BOX_SOURCE" --cache "$BOX_CACHE" sync
bdev --host "$BOX_LOGIN" --source "$BOX_SOURCE" --cache "$BOX_CACHE" build bobj
```

Serialize syncs and builds for each checkout on the box. Run long syncs
as a detached Mac process with a log. The source directory must be
dedicated to the sync. Use `sync --delete` when a dedicated mirror needs
to reflect removed or renamed source files. This authorizes removal of
unexcluded paths inside the listed roots only. Without that flag,
preflight refuses plans that require removal. Sync skips bsync's reserved
`.bsync-*` staging paths, so interrupted uploads can be resumed while
their temporary files remain untouched.

After preflight, the driver removes the previous tree stamp before
applying any changes. If an interruption leaves no stamp, builds refuse
until a sync completes.

The sync stages tracked and unignored files from the allowlisted roots
and uses the bsync engine once per root. `sync.json` lists the roots and
exclusions. When the userland workspace exists, Cargo metadata adds its
repository-local library dependencies. After all roots have transferred,
the driver publishes `.btest-tree.json` atomically via `bput`. That stamp
records the commit, local `origin/main` ref, ancestry and dirty state,
and one SHA-256 per root. Each root digest covers the sorted bsync paths,
modes, file sizes and file digests.

The build command uses the E25 self-host recipe for packages in
`toolchain/`. It reads `/opt/breenix/rust/CURRENT` on the box and uses
the selected x86-64 Cargo, compiler, sysroot and linker by full path.
The command writes the build diagnostics to stderr and a single JSON
result to stdout. The result includes the SSH elapsed time and the box's
elapsed time when reported by `time`. For packages with E25 output pins,
it includes and checks the on-box artifact SHA-256. The pinned bobj
artifact is `libbobj.rlib`.

Run the Mac checks with `cargo test --locked --manifest-path tools/bdev/Cargo.toml`.
