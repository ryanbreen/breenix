# BusyBox built with Breenix libc

BusyBox 1.37.0 is built by `scripts/build-libc-busybox.sh` using `scripts/breenix-cc`
and Breenix's own headers and `libc.a`. `userspace/programs/build.sh` builds it on
both architectures; no prebuilt binary is installed.

The complete upstream source is `busybox-1.37.0.tar.bz2`, from
https://busybox.net/downloads/busybox-1.37.0.tar.bz2, SHA-256
`3311dff32e746499f4df0d5df04d7eb396382d7e108bb9250e7b519b837043a4`.
BusyBox is GPLv2; `LICENSE` and the archive contain its license notices.
`breenix-libc.config` contains the configuration. Small POSIX corrections live
in `patches/`, applied in `patches/series` order after extraction. The source
checksum, configuration, patches, headers and library enter the build digest.

Outputs are `libc-busybox.elf`, `libc-busybox.links`, and per-applet compile/link
results at `target/libc-busybox/<arch>/applets.json`. Every configured applet must
compile and link; `required-applets` also preserves every legacy command name
and every suite applet. Missing names fail before image creation.

`scripts/install-libc-busybox.sh` serves both image creation paths. `/bin/busybox`,
all `/bin` applets, `/sbin/true`, `/sbin/false`, and `/usr/local/libc/bin/busybox`
and its applet links share one inode. `/bin/ls` is the BusyBox applet on both
architectures. C utilities under `userspace/utilities/*.c` are built by the same
C driver and installed in `/bin`.
