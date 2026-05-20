# Turn 37 validation: BLOCKED

## Status

BLOCKED against the T37 completion criteria. The build was clean and the single
ARM64 QEMU boot confirmed the MMIO GPU path, but the boot did not reach the
60-second health threshold. It launched `/sbin/init`, then CPU0 repeatedly
reported `UNHANDLED_EC` at `ELR=0xffff0000400fc338`.

No source files were changed.

## 37A T36 commit state

Already satisfied before the T37 build/boot work started:

- `b2aca1c3 docs(polling): turn 36 P9 diagnostic — gpu_mmio reachability + CPU0 regression bisect`

No duplicate T36 commit was created.

## Build result

Completed:

- `turn37-artifacts/build-userspace.log`
- `turn37-artifacts/build-ext2.log`
- `turn37-artifacts/build-aarch64.log`

The warning/error grep is 0 bytes:

- `turn37-artifacts/build-aarch64-warning-error-grep.txt`

## Single QEMU boot result

Command shape was the native ARM64 QEMU `virt` path with one launch, no wrapper
retry:

- `-M virt -cpu max -m 512 -smp 4`
- `-device virtio-gpu-device`
- `-device virtio-keyboard-device`
- `-device virtio-tablet-device`
- `-device virtio-blk-device`
- `-device virtio-net-device`

Artifacts:

- `turn37-artifacts/boot-1-run.out`
- `turn37-artifacts/boot-1-serial.log`
- `turn37-artifacts/boot-1-exit-code.txt` (`124`, timeout after the preserved
  single run)
- `turn37-artifacts/boot-1-first-exception-window.txt`
- `turn37-artifacts/boot-1-first-exception-window-numbered.txt`

The boot did not succeed past the health threshold. There were no heartbeat
uptime lines. Serial line 167 launches init, line 168 starts process creation,
line 178 shows the first `UNHANDLED_EC`, and line 179 emits
`[FATAL_POSTMORTEM] cpu=0 label=UNHANDLED_EC`
(`turn37-artifacts/boot-1-first-exception-window.txt`). The exception scan has
434 lines (`turn37-artifacts/boot-1-exception-scan.txt`).

The directive's fail scan is nonzero:

- `turn37-artifacts/boot-1-fail-scan.txt` is 331 bytes.

That file includes benign boot messages such as missing VirtIO sound/AHCI plus
PSCI CPU probe failures. The real fatal marker is captured separately in
`boot-1-exception-scan.txt` because the directive's fail regex did not include
`UNHANDLED_EC`.

## Did init_virtio_mmio() run?

Yes. The QEMU boot used the hybrid path and enumerated VirtIO MMIO devices:

- line 35: `[drivers] Hybrid mode: VirtIO MMIO + PCI AHCI`
- lines 36-40: network, block, input, input, and GPU VirtIO MMIO devices found
- line 41: `[drivers] Found 5 VirtIO MMIO devices`
- line 74: `[drivers] Driver subsystem initialized (MMIO)`

See `turn37-artifacts/boot-1-mmio-bus-walk.txt`.

## Did gpu_mmio::init() succeed?

Yes. The MMIO GPU initialized successfully:

- line 63: `[virtio-gpu] Searching for GPU device...`
- line 64: `[virtio-gpu] Found GPU device at 0xa003e00`
- line 66: `[virtio-gpu] Control queue max size: 1024`
- line 67: `[virtio-gpu] Display: 1280x800`
- line 68: `[virtio-gpu] GPU device initialized successfully`
- line 69: `[drivers] VirtIO GPU driver initialized`
- line 71: `[virtio-gpu] Test passed!`

See `turn37-artifacts/boot-1-mmio-gpu-init.txt`.

## Was the gpu_mmio.rs:519 spin reached?

Reached by inference, not by direct serial marker. Current `gpu_mmio.rs` does
not log `GET_DISPLAY_INFO`, `send_command()`, or `CTRL_QUEUE` activity, so
`turn37-artifacts/boot-1-gpu-command-evidence.txt` is 0 bytes.

The source path is direct:

- `kernel/src/drivers/virtio/gpu_mmio.rs:425-451` prepares
  `GET_DISPLAY_INFO` and calls `send_command()`.
- `kernel/src/drivers/virtio/gpu_mmio.rs:475-535` posts the descriptors and
  waits in the spin loop at line 519 until `used.idx` advances or timeout.
- `kernel/src/drivers/virtio/gpu_mmio.rs:530-531` would return
  `GPU command timeout` on failure.

The serial log shows `[virtio-gpu] Display: 1280x800` and
`[virtio-gpu] GPU device initialized successfully`
(`turn37-artifacts/boot-1-mmio-gpu-init.txt:5-6`), and
`turn37-artifacts/boot-1-gpu-timeout-scan.txt` is 0 bytes. Therefore the
`GET_DISPLAY_INFO` command completed, which requires the `send_command()` wait
loop to observe `used.idx` advancing at least once.

## Proposed next step

Do not start the P9 implementation on top of this baseline yet. T37 confirmed
the correct target and the MMIO GPU spin path, but the QEMU userspace baseline
is not healthy. The next turn should first decide whether to:

1. unblock or scope down the QEMU health failure (`UNHANDLED_EC` after init
   launch), then rerun the baseline; or
2. accept the reachability/spin evidence as sufficient and proceed with a
   strictly MMIO-local observer.

If proceeding with P9 despite the health failure, the smallest implementation
change remains: add IRQ identity/state and an unwired handler inside
`gpu_mmio.rs` only. Do not touch `exception.rs`; T36 identified that
Parallels-visible dispatch branch as the T35 CPU0 regression suspect.
