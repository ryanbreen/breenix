# Round-2 serials, aarch64 `testing` profile

Five captures. Each was taken with the soft-float kernel target
(`aarch64-breenix-kernel.json`, `-Z build-std=core,alloc -Z
build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
--features testing`) and this QEMU line, from a fresh copy of
`target/ext2-aarch64.img` per boot:

```
qemu-system-aarch64 -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
  -kernel target/aarch64-breenix-kernel/release/kernel-aarch64 \
  -display none -no-reboot \
  -device virtio-gpu-device -device virtio-keyboard-device \
  -device virtio-tablet-device \
  -device virtio-blk-device,drive=ext2 \
  -drive if=none,id=ext2,format=raw,file=<copy> \
  -device virtio-net-device,netdev=net0 -netdev user,id=net0 \
  -serial file:<out>
```

| File | What it is |
|---|---|
| `testing-profile-boot.txt` | The round-2 acceptance boot. `SOFTIRQ_TEST: iteration limit passed (25 total iterations, ksoftirqd/1)`, `[test] Loaded 78/78 test binaries (0 failed, 0 not found)`, `[test] Test processes loaded - will run via timer interrupts`; 0 `IDLE_SLEEP_REFUSED` lines and 0 `PINNED_HOME_CPU_UNAVAILABLE` lines. The post-loader soft lockup is present and is described in `TESTING-PROFILE-REVIVAL-2026-09-04.md`. |
| `testing-profile-boot-at-06d149b6.txt` | The same profile at the round-1 branch tip, against the regenerated fixture. `[test] Loaded 78/78` -- the round-1 doc's number is reproducible once the fixture matches the catalog; the reviewer's 77/78 came from an ELF set that predated the catalog's 78th entry. |
| `idle-refusal-before-the-loader-moved.txt` | The shared idle refusal in place but the loader still on the boot identity: one `[IDLE_SLEEP_REFUSED:first:count=1]`, during the pre-timer `/sbin/init` pre-load. This is the counter doing its job. |
| `loader-kthread-masked-boot-cpu.txt` | The loader in a kernel thread with the boot CPU still masked: `[test] Loaded 0/78 test binaries (0 failed, 78 not found)`. The instrumented run behind it reported `read_sector(522) -> Block MMIO read timeout` and then `device wedged after an abandoned request`. This is why the boot CPU is unmasked before the loader spawns. |
| `pinned-home-park-before-the-offline-narrowing.txt` | `[PINNED_HOME_CPU_UNAVAILABLE:first:tid=2:home=0:cpu=3:count=1]` on a boot with nothing else wrong: CPU 3's reclaim pass parking `ksoftirqd/0` because the deliberately non-preemptible boot CPU reads as stalled. This is why reclaim now parks only on an OFFLINE home. |
