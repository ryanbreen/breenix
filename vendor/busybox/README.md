# BusyBox disk artifacts

Every ext2 disk installs these pinned BusyBox 1.37.0 binaries, including fresh
beast gate clones. `manifest.json` pins their SHA-256 and ELF machine type;
`scripts/install-busybox.py` verifies both before disk creation. Installation
fails if an artifact is absent or corrupt rather than omitting coreutils.

These static, stripped binaries were built with the repository's
`scripts/build-busybox.sh` and `breenix.config`, using the Homebrew musl-cross
GCC 14.2.0 compilers (`x86_64-linux-musl-gcc` and `aarch64-linux-musl-gcc`).
The recipe downloads BusyBox's corresponding source from
https://busybox.net/downloads/busybox-1.37.0.tar.bz2 and checks SHA-256
`3311dff32e746499f4df0d5df04d7eb396382d7e108bb9250e7b519b837043a4`.
It fixes the build timestamp and maps text at `0x40000000`.
BusyBox is GPLv2; its license is included in `LICENSE`, and the source archive
contains the complete upstream source and license notices. Breenix changes
only the configuration, included here.

To rebuild an artifact with those compilers:

```sh
export CARGO_BUILD_JOBS=6
scripts/build-busybox.sh --arch x86_64
scripts/build-busybox.sh --arch aarch64
```

The resulting files are `userspace/programs/busybox.elf` and
`userspace/programs/aarch64/busybox.elf`. To update the pinned artifacts, copy
those outputs here, recompute the manifest digests, and commit the binaries,
configuration and digests together. Disks always install the pinned version;
an unpinned local build is not silently substituted.

On x86 `/bin/ls` remains a BusyBox hardlink. ARM64 retains the existing native
`bls` replacement for `/bin/ls` while #1074's runtime faults remain unresolved.
Provisioning BusyBox on ARM64 prevents missing-binary failures in fresh clones;
it does not claim to repair those runtime faults.
