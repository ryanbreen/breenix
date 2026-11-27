# ARM64 QEMU rig

`rig.py` runs bbench on Breenix and on a stock Linux in the same ARM64 QEMU VM
shape on this Mac (DEV-31). Each run writes two schema-v1 series,
`<run>-linux` and `<run>-breenix`, with target `arm64-qemu`, to
`~/Library/Caches/breenix-devbox/bench-series/`. Each row's Breenix/Linux
ratio judges the kernel mechanism behind it.

## The shape

Both guests use the repository's standard ARM64 configuration (`run.sh` and the
production-profile boot test), at the memory the compile rows' suites declare:

- `qemu-system-aarch64 -M virt,gic-version=3`, accelerated by HVF with
  `-cpu host` through `scripts/qemu-arm64.py`, with 4 vCPUs;
- 2048 MiB. The default boot has 512 MiB, but `docs/suites/*.json` declare a
  `minimumGuestMemory` of 1024 MiB for rustc and cargo and 2048 MiB for the
  toolchain effort's self-host acceptance. The rig takes the largest of the
  three, so neither system's page cache is held below that standard;
- two 8192 MiB raw disks on `virtio-blk-device`, both `format=raw,cache=writeback`
  (QEMU's default): the root disk, and the disk rows' volume, listed first;
- `virtio-net-device` on user networking, with SSH forwarded from a loopback
  port;
- `virtio-rng-device` and the virtio GPU, keyboard and tablet devices.

The guests differ only in what they boot. Breenix boots its production kernel with
`-kernel`. Linux boots the distribution's own kernel through QEMU's edk2
firmware.

## Inputs

Breenix comes from this checkout at the commit being measured. Build it first,
from a committed tree, with the hosted toolchain's build caches named the way
the rustc and cargo suite boots name them:

```sh
BREENIX_RUSTC_PORT_BUILDS=$HOME/Library/Caches/breenix-e25/ak5 \
BREENIX_K7_STAGE=<K7 stage directory holding aarch64/bin/cargo> \
    scripts/bench-qemu-arm64.sh build-breenix
```

`BREENIX_RUSTC_PORT_BUILDS` is the ARM K5 cache (`hosted/`, `sysroot/` and
`shared-libc/libc.so`); `BREENIX_K7_STAGE` holds the ARM K7 Cargo whose digest
`toolchain/rust-host/k7/aarch64.json` pins. `build-breenix` runs the builds the
`rustc,cargo` suite boot runs (`scripts/boot-interactive.sh`):

```sh
userspace/programs/build.sh --arch aarch64
userspace/programs/build.sh --arch aarch64 --bbench
userspace/programs/build.sh --arch aarch64 --rust-link
userspace/programs/build.sh --arch aarch64 --hosted-bin suite-rustc --features rustc
userspace/programs/build.sh --arch aarch64 --cargo-suite
userspace/programs/build.sh --arch aarch64 --rustc
userspace/programs/build.sh --arch aarch64 --kd-loader
scripts/create_ext2_disk.sh --arch aarch64
cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
cargo build --release --locked --manifest-path tools/breenfs/Cargo.toml \
    --bin breenfs-mkfs --bin breenfs-check
```

It then copies `target/ext2-aarch64.img` to
`~/Library/Caches/breenix-bench/breenix-arm64/root.raw`, stages the hosted
toolchain into that copy with `scripts/rust_host_stage.py --suite rustc,cargo`
(the suites' own staging, which checks each file against the K5 and K7 build
records), and grows it to 8192 MiB. It writes
`~/Library/Caches/breenix-bench/breenix-arm64/build.json` with the commit and the
SHA-256 of the kernel, the staged image, the four bbench binaries and the two
BreenFS tools. `run` refuses to start unless that stamp names HEAD and each
artifact still has its stamped digest; the series' kernel and userland commit is
the stamped one (`test_rig.BuildIdentity`). Linux builds bbench from the same
commit (`git archive` of the stamped commit).

The key directory keeps the image's test-only SSH key, so the rig can log in as
`sshtest`. `--bbench` builds `bbench` and `bbench-exit` as shipped (opt-level z,
with libbreenix at 3), and for the crypto rows `bbench` twice more: with
libbreenix at opt-level z (profile `bench-oz`), and at opt-level 3 throughout.
The rig uploads those four binaries over SSH.

`linux-arm64.json` pins Linux: the Ubuntu 24.04 LTS arm64 cloud image from a
dated release directory and its SHA-256 from that release's `SHA256SUMS`.
`fetch-linux` downloads it into `~/Library/Caches/breenix-bench/linux-arm64/`
and refuses any copy whose digest differs, whether downloaded or cached. It then
converts the image to raw at the rig's disk size. `provision-linux` boots
the image once with a cloud-init seed. It creates the `bench` account, installs
`gcc`, and installs rustup's `nightly-2025-06-24`, the upstream commit
706f244db that the Breenix Rust fork (a763a7dd7) builds on. It then turns
off the apt timers and cloud-init. The rig leaves the kernel as shipped: Linux is a
black box that we boot, run and time.

## Running

```sh
scripts/bench-qemu-arm64.sh run --run qemu-20261127-main
```

The launcher takes the Mac's boot slot and the host-wide ARM64 QEMU lock. It
waits for a quiet host, then runs `rig.py`. The rig boots both guests and keeps
both up. For each case it takes sample 1 on Linux, then on Breenix, then sample
2 on each, and so on, so host drift lands on both systems. `--count` sets the
samples per case (default 5), `--cases` selects a subset, and `--headed` shows
the VMs in windows. Receipts, serial logs and QEMU command lines go to
`~/Library/Caches/breenix-bench/runs/<run>/`. The rig stops only the QEMU
processes it started, by the pid it recorded.

## Rows

The rig runs bbench's default set in each guest.

Compile rows run on both systems, each with its own toolchain for its own
native target. Linux uses the pinned `nightly-2025-06-24`. Breenix uses the
hosted build of the fork at `/usr/libexec/breenix/rustc`, with the
`aarch64-breenix` target and the settings of the cargo suite's invocation
(`RUSTC`, the sysroot, `bld`, `libc.a` and `linker.ld`). Both build with
`CARGO_BUILD_JOBS=4`. The Breenix series' toolchain field and note record that it
runs the fork, 16 commits ahead of Linux's nightly. The rig does not yet build the
toolchain workspace, so the four self-host rows are deferred on both systems.

The disk rows (sequential, small-files, walk and fsync) run on the filesystem
each system targets, on the second disk:

- Breenix: BreenFS, made on the host by our `breenfs-mkfs` with a `bench`
  directory owned by `sshtest`, checked with `breenfs-check --full`, and mounted
  by the kernel at `/home` (ARM64 mounts block device 1 there);
- Linux: ext4, made in the guest by its own `mkfs.ext4` with default options and
  mounted at `/mnt/bench`.

Setup refuses the run unless the guest reports the volume as `breenfs` or `ext4`,
read-write. Each disk row's `cache` field names its filesystem. The driver-map
rows map a 64 MiB file that bbench writes in each guest on the root disk, so both
systems map the same number of bytes. The SCP rows move 8 MB.

The audit's attribution rows (step 0.5) run on both systems:

- getpid ×1M;
- 64-byte malloc/free ×1M on 1 and 4 threads;
- raw TCP of 32 MB in each direction between guest and host;
- AES-256-GCM and HMAC-SHA256 from libbreenix's crypto, built at opt-level z
  and at opt-level 3.

## Series and provenance

`vigil-cli breenix bench record` refuses a series whose text fields break its
rules: hardware at most 200 characters, kernelCommit and userlandCommit 64,
toolchain 200, session 120, note 400, a row's title 200, cache 300 and reason
400, each a single line with no IPv4 literal, host name, URL or credential-like
text. `vigil_rules.py` ports those checks and `series.write_series` fits the 9
text fields to them, replacing addresses and hosts and shortening with an ellipsis.
The full provenance goes to `<id>.provenance.json` beside each series: the image
URL and SHA-256, the full `uname`, the full VM shape, the bench volume's mount
line, the TCP retransmission counters at boot and after the network rows, the
restart notes, and the original of any shortened field. The note names that file.
`test_vigil_limits` writes both series from a run with long provenance and
checks them against those rules.
