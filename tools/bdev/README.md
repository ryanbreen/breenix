# On-box builds and tests

Install the Mac driver with `cargo install --locked --path tools/bdev`.
It is written in Rust to reuse the existing source snapshot and bsync engine.

For tests, create `~/.config/breenix-devbox/bdev.json` with the alias from
your OpenSSH config and the absolute path to the dedicated source mirror
on the box. Replace the example path with your remote home path.

```json
{
  "host": "devbox",
  "source": "/REMOTE/HOME/src/breenix"
}
```

Set `"jump": "bastion"` to use the specified OpenSSH bastion alias via
ProxyJump, or set ProxyJump in the alias's OpenSSH config. An alternate
OpenSSH configuration file can be specified with `ssh_config` in the
JSON. To use a different bdev configuration, pass `--config FILE` before
`test`. By default bdev finds the JSON-capable btest on the SSH exec PATH.
For a bootstrapped copy in `~/bin`, set `"btest_path": "/REMOTE/HOME/bin/btest"`
using its absolute path on the box.

From a Breenix checkout:

```sh
bdev test yes
bdev test --changed
```

`--changed` uses `git diff --name-only origin/main...HEAD` and includes
uncommitted paths. Mac-side Cargo metadata maps those paths to suites
and follows transitive path dependencies. Workspace manifest, lockfile
and bu-sys changes select the full set of available suites. With no selected suites,
the command exits successfully without connecting. The explicit form
accepts multiple packages and sends them in one btest invocation.

The test command holds a local workspace lock and uses a single SSH
ControlMaster for bsync, stamp publication and btest. It mirrors
deletions inside the listed roots, streams JSON case lines and the final
summary, and saves the summary to
`~/Library/Caches/breenix-devbox/runs/<utc-seconds>-<nanoseconds>.json`.
The exit status comes from btest. Cargo builds and cases run on the box.

For standalone source sync and self-host builds, specify the SSH login,
the absolute source directory on the box and a separate remote cache.

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
