# Install programs on a Breenix disk image

`breenix-image` edits an unmounted, raw ext2 filesystem image. Supply the
filesystem image itself, not a partitioned disk or a mounted filesystem. Stop
any VM using it and keep a backup. The tool needs Python 3.9 or newer and
e2fsprogs debugfs. It searches `BREENIX_DEBUGFS`, Homebrew's e2fsprogs keg,
`/sbin`, `/usr/sbin`, then PATH. Keep enough host space for one image copy:
edits publish atomically after verification and fsync. Host ownership, permissions
and extended attributes are preserved. Atomic publication changes the image inode;
hard-linked images are refused, so first make an independent copy.

## Install

```sh
breenix-image install --image "$BREENIX_IMAGE" "build/$BREENIX_ARCH/my-app"
```

This installs the static ELF as `/usr/local/bin/my-app`, with mode 0755 and
owner root:root. `--name NAME` selects a different destination basename;
`--mode OCTAL` selects permissions, for example `--mode 0700`. Names must be
single basenames, not paths. Missing installation directories are created with
mode 0755 and owner root:root. Reinstalling replaces an existing SDK-managed regular file; unmanaged files
are refused. Existing directory ownership and permissions are preserved.

The tool reads back installed bytes and metadata before publishing the edit.
It derives the disk's architecture from `/bin/busybox` and refuses a non-ELF,
wrong-architecture ELF, ET_DYN image or executable containing `PT_INTERP` without changing
the image. ARM64 programs work on ARM64 QEMU, Parallels and VMware images;
x86-64 programs need an x86-64 image.

## List and remove

```sh
breenix-image list --image "$BREENIX_IMAGE"
breenix-image remove --image "$BREENIX_IMAGE" my-app
```

List prints sorted SDK-managed names. The registry is
`/usr/local/lib/breenix-sdk/installed` inside the image. Remove refuses an
unregistered or missing name. `breenix-image --version` reports the SDK version
and source commit.

## Boot the edited image

The SDK installs applications; the separate Breenix checkout supplies the
kernel and VM launchers. Set `BREENIX_CHECKOUT` and `BREENIX_IMAGE` to absolute
paths. Use the architecture matching the edited image:

| Target | Invocation |
| --- | --- |
| ARM64 QEMU | `"$BREENIX_CHECKOUT/run.sh" --disk "$BREENIX_IMAGE"` |
| x86-64 QEMU | `"$BREENIX_CHECKOUT/run.sh" --x86 --disk "$BREENIX_IMAGE"` |
| Parallels | `"$BREENIX_CHECKOUT/run.sh" --parallels --disk "$BREENIX_IMAGE"` |
| VMware | `"$BREENIX_CHECKOUT/run.sh" --vmware --disk "$BREENIX_IMAGE"` |

`--disk` uses a private boot copy without rebuilding the disk's contents.
Add `--no-build` to skip kernel and loader compilation too. Boots run at normal
priority through the shared boot slot. At the guest shell, execute
`/usr/local/bin/my-app`, or `my-app` when `/usr/local/bin` is on PATH.

See the [quick start](quick-start.md) for a complete create/build/install/run
example and the [reference](reference.md) for compiler and library details.
