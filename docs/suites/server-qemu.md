# ARM64 QEMU server captures

Create a private account file with `scripts/vm_site.py operator-account --out FILE`.
For evidence, build from a clean checkout and omit `--system-image`:

```sh
export CARGO_BUILD_JOBS=4
scripts/vm-persistent.sh qemu build --run-dir PRIVATE_RUN --operator-account PRIVATE_ACCOUNT
scripts/vm-persistent.sh qemu boot --run-dir PRIVATE_RUN --mode console
```

The source build runs the production ARM kernel and userspace builders, packs a
new system image, stages the private account, and seals SHA-256 digests of the
kernel and both disks. Boot verifies those digests against the clean tools
commit and copies the selected kernel into the run before launching QEMU.
The boot records retain the artifact digests. Preserve `build.json`,
`verified-build.json`, and `boots/` with the capture before deleting the run.

An explicit `--system-image` imports an arbitrary image for diagnostics. Imported
runs are evidence-ineligible and have no asserted source commit. A source run's
modified disks cannot be launched as the original sealed artifacts again; use
the same QEMU process for persistent reboot cases, or build a fresh run.

Use `--mode console` for server cases; desktop mode starts graphical services
without the server daemon and login table. The launcher owns the Mac boot slot
and opens its visible display. Stop only a run this session owns, after preserving
its capture, with `scripts/vm-persistent.sh qemu stop --run-dir PRIVATE_RUN`.

The offline auditor's inactive-run receipt tests lease refusal with no live
machine. It does not isolate the ended marker. The target's focused host checks
exercise that predicate while satisfying the other lease conditions.
