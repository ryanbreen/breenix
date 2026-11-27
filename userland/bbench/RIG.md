# ARM64 QEMU rig

`rig.py` runs bbench on Breenix and on a stock Linux in the same ARM64 QEMU VM
shape on this Mac (DEV-31). Each run writes two schema-v1 series,
`<run>-linux` and `<run>-breenix`, with target `arm64-qemu`, to
`~/Library/Caches/breenix-devbox/bench-series/`. Each row's Breenix/Linux
ratio judges the kernel mechanism behind it.

## The shape

Both guests use the repository's standard ARM64 configuration (`run.sh` and the
production-profile boot test):

- `qemu-system-aarch64 -M virt,gic-version=3`, accelerated by HVF with
  `-cpu host` through `scripts/qemu-arm64.py`, with 4 vCPUs and 512 MiB;
- one 8192 MiB raw disk on `virtio-blk-device` at QEMU's default cache mode
  (writeback);
- `virtio-net-device` on user networking, with SSH forwarded from a loopback
  port;
- `virtio-rng-device` and the virtio GPU, keyboard and tablet devices.

The guests differ only in what they boot. Breenix boots its production kernel with
`-kernel`. Linux boots the distribution's own kernel through QEMU's edk2
firmware. The `hardware` field of each series records the shape.

## Inputs

Breenix comes from this checkout at the commit being measured. Build it first,
from a committed tree:

```sh
scripts/bench-qemu-arm64.sh build-breenix
```

That runs these builds, then writes `~/Library/Caches/breenix-bench/breenix-arm64/build.json`
with the commit and the SHA-256 of the kernel, the image and the four bbench
binaries:

```sh
userspace/programs/build.sh --arch aarch64
userspace/programs/build.sh --arch aarch64 --bbench
BREENIX_TEST_SSH_KEY_DIR=$HOME/Library/Caches/breenix-bench/breenix-arm64/test-ssh \
    scripts/create_ext2_disk.sh --arch aarch64 --size 8192
cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc \
    -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

`run` refuses to start unless that stamp names HEAD and each artifact still has
its stamped digest, so a series never names a commit its Breenix binaries were
not built from. Running the builds by hand leaves no stamp. Linux builds bbench
from the same commit (`git archive` of the stamped commit).

The key directory keeps the image's test-only SSH key, so the rig can log in as
`sshtest`. `--bbench` builds `bbench` and `bbench-exit` as shipped (opt-level z,
with libbreenix at 3), and for the crypto rows `bbench` twice more: with
libbreenix at opt-level z (profile `bench-oz`), and at opt-level 3 throughout.
The rig uploads those four binaries over SSH, so the system image keeps its
standard contents. The Linux guest builds the same three profiles.

`linux-arm64.json` pins Linux: the Ubuntu 24.04 LTS arm64 cloud image from a
dated release directory and its SHA-256 from that release's `SHA256SUMS`.
`fetch-linux` downloads it into `~/Library/Caches/breenix-bench/linux-arm64/`
and refuses any copy whose digest differs, whether downloaded or cached. It then
converts the image to raw at the rig's disk size. `provision-linux` boots
the image once with a cloud-init seed. It creates the `bench` account, installs
`gcc`, and installs rustup's `nightly-2025-06-24`, the upstream commit
706f244db that the Breenix Rust fork (a763a7dd7) builds on. It then turns
off the apt timers and cloud-init. The rig leaves the kernel as shipped: Linux is a
black box that we boot, run and time. Inside the guest, bbench is built from the
same checkout with that compiler. The same nightly builds Breenix's userspace.

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

The rig runs bbench's default set in each guest. DEV-23's ban on disk rows covers
the R240 only. In this rig, the disk rows run on the root virtio disk: ext2 on
Breenix and ext4 on Linux. The driver-map rows map a 64 MiB file written in the
guest, because the ARM image has no compiler driver. The SCP rows move 8 MB.

The audit's attribution rows (step 0.5) run on both systems:

- getpid ×1M;
- 64-byte malloc/free ×1M on 1 and 4 threads;
- raw TCP of 32 MB in each direction between guest and host;
- AES-256-GCM and HMAC-SHA256 from libbreenix's crypto, built at opt-level z
  and at opt-level 3.

After the network rows, the series note records each system's retransmission
counters: Linux's `RetransSegs` and `TCPTimeouts`, and Breenix's
`retransmit_timeouts` from `/proc/net/tcp_stats`.

Breenix's standard ARM image has no hosted rustc or Cargo, so the compile rows run
on Linux only. On Breenix they are deferred, with that reason. The rig does not yet
build the toolchain workspace, so the four self-host rows are deferred on both
systems. A deferred row has no median and plots as no data.
