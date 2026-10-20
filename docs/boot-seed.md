# VMware boot entropy

The shared ARM64 UEFI loader tries EFI_RNG_PROTOCOL, then TPM2_GetRandom,
then `\EFI\BREENIX\random-seed` on the ESP it was loaded from. The firmware
sources retain their priority, so Parallels keeps using its TPM. QEMU's
direct kernel boot and virtio-rng path are unchanged.

Before VMware power-on, including `--no-build`, `run.sh` invokes
`scripts/vmware/boot-seed.py` to read 64 fresh bytes from the host's
`/dev/urandom`. It injects those raw bytes into the per-run VMDK's ESP.
The cached, reusable boot image stays seed-free. Seeding failure
stops the launcher before power-on. This requires mtools (`mcopy`) as well
as the existing `qemu-img` dependency.

The loader accepts exactly 64 bytes, overwrites the entire file with the
`BREENIX-CONSUMED` prefix and NUL padding, and flushes it before releasing
the seed to the existing HardwareConfig v7 handoff. Missing, short,
oversized, consumed, read-only, or failed-write/flush files earn no credit.
A later boot of that disk without the launcher sees the consumed mark.
The kernel credits the host seed through the same pool initialization as
firmware seeds, wipes the handoff copy, and reports `boot_seed=host-file`
and `boot_seed_bits=512`. The loader reports `[entropy] boot seed: host-file`
on serial. No source means no boot-seed credit; timer jitter remains uncredited.

The design reference is [systemd-boot's documented ESP seed handoff](https://www.freedesktop.org/software/systemd/man/260/systemd-boot.html).
This is a clean-room implementation of S43: the host refreshes each seed,
and the loader consumes it rather than deriving a replacement. It relies
on successful firmware file flushes for persistence; restoring a snapshot
of an unconsumed disk also restores its seed, so only seed-free cached
images should be distributed or cloned.
