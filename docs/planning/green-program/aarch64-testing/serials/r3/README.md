# Round-3 serials, aarch64 `testing` profile

2 files (`ls docs/planning/green-program/aarch64-testing/serials/r3/*.txt |
wc -l` -> 2). Both were captured with the soft-float kernel target
(`aarch64-breenix-kernel.json`, `-Z build-std=core,alloc -Z
build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
--features testing`) and this QEMU line, from a fresh copy of
`target/ext2-aarch64.img` per boot, 3 boots at a time on the same host:

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
| `testing-profile-boot.txt` | The round-3 acceptance boot, boot 1 of the 12-boot `c2A` batch. 7754 lines. `SOFTIRQ_TEST: iteration limit passed`, `[test] Loaded 78/78 test binaries (0 failed, 0 not found)` and `[test] Test processes loaded - will run via timer interrupts` at line 1924, then userspace, then the post-loader #728 soft lockup 1517 lines later. `grep -c EXT2_LOCK_SPIN_STALL` -> 9 (7 `ROOT_EXT2_read`, 2 `ROOT_EXT2_write`); `grep -c "^\[test\] Loaded [a-z]"` -> 78. |
| `test7-wedge-before-the-halt-fix.txt` | A wedged boot from the middle row of the measurement table: the boot sequence on a kernel thread, the park protocol in place, but `kthread_join` still halting with interrupts masked. `[WORKQUEUE_PARK_RACE:cancelled=0:intent_cleared=0]` is the last non-heartbeat line; `grep -c SOFTIRQ_TEST` -> 0, so it stopped inside Test 7's daemon phase. Kept because it is the failing serial the round's halt fix is aimed at. |
