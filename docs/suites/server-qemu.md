# ARM64 QEMU server captures

Create a private account file with `scripts/vm_site.py operator-account --out FILE`.
For evidence, build from a clean checkout and omit `--system-image`:

```sh
export CARGO_BUILD_JOBS=4
scripts/vm-persistent.sh qemu build --run-dir PRIVATE_RUN --operator-account PRIVATE_ACCOUNT
scripts/vm-persistent.sh qemu boot --run-dir PRIVATE_RUN --mode console
```

Run the operator-login milestone against the booted run:

```sh
scripts/metal/server-suite --target qemu --run-dir PRIVATE_RUN --boot BOOT_ID --sha COMMIT --milestone users/operator-login
```

The QEMU backend uses the run's serial console for the operator challenge.

For the reboot suite, add `--fault panic-reboot` to the build command. It seals
a separate fault kernel for `fault-boot` and restores the production kernel at
the shared build path before building the image. Fault launch verifies the
sealed fault kernel digest as well as the source commit.

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

## BreenFS upgrades and ext2 retirement

Build a fresh source-attested run and finish its disposable VM migration
through the verified return to BreenFS. Pass `--no-display` on each migration
boot for a headless run with one-minute host load at or below 8. Use the same
run directory, operator identity and trusted guest host key throughout retirement.

```sh
scripts/vm-persistent.sh qemu begin-retirement --run-dir "$run"
scripts/vm-persistent.sh qemu install --run-dir "$run"
scripts/vm-persistent.sh qemu boot --run-dir "$run" --mode console --slot next
```

During the verification boot, run `probe-migration` with the operator
identity and trusted host key from another terminal, then request guest
poweroff. The launcher promotes NEXT after it verifies the generation,
BreenFS mounts, unchanged home sentinel and authenticated shutdown. Repeat
the install and NEXT verification once. Each install copies the sealed
staging image into a new generation.

```sh
scripts/vm-persistent.sh qemu reclaim-ext2 --run-dir "$run"
scripts/vm-persistent.sh qemu boot --run-dir "$run" --mode console --slot current
```

Probe the final boot and request guest poweroff in the same way. The run
directory retains `filesystem-two-upgrades.json`, `filesystem-reclaimed.json`,
its boot captures and `retirement.img`. The last record includes the offline
CURRENT selection and intent digest for the next launch.

QEMU receives bounded views of the BreenFS partitions on the disposable GPT
image and a read-only staging partition during install. The stopped host edit
checks both GPT copies against the packed layout, clears entries 3 and 4 and
reads both copies back. The retired ext2 data blocks retain their bytes.
Retirement launches run headless with the Mac boot slot.
