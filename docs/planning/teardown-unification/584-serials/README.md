# #584 B1 production-profile negative-control baseline

This directory records the red/green baseline for the #584 B1 production-profile negative control.

## Red: pre-fix untimed driver

The mutation restored only `userspace/programs/src/futex_handoff_oracle.rs` from commit `69d57646`. That driver has no stage-0 arming handshake and passes `timeout = 0` to its blocking futex stages.

The command was:

```text
./docker/qemu/run-aarch64-prod-profile-boot-test.sh --rebuild-userspace
```

It exited 1 with this assertion:

```text
FAIL: seam-absent timeout marker count must be exactly one
```

The process-launch portion of `prod-profile-mutation-red-untimed-driver.txt` shows that PID 6 was spawned, with no subsequent output from the oracle and no init exit line:

```text
[spawn] path='/bin/futex_handoff_oracle'
[heartbeat] tid=9 uptime_ms=5459 kbd_nonzero=0
manager.create_process_with_argv [ARM64]: ENTRY - name='futex_handoff_oracle', elf_size=292040, argc=1
manager.create_process_with_argv [ARM64]: Generated PID 6
manager.create_process_with_argv [ARM64]: Creating ProcessPageTable
manager.create_process_with_argv [ARM64]: Loading ELF into page table
manager.create_process_with_argv [ARM64]: ELF loaded, entry=0x4000ee84
manager.create_process_with_argv [ARM64]: Allocating user stack
manager.create_process_with_argv [ARM64]: User stack will be at 0xfffffeff0000-0xffffff000000
manager.create_process_with_argv [ARM64]: argc/argv set up on stack, SP=0xfffffefffec0
manager.create_process_with_argv [ARM64]: SUCCESS - returning PID 6
[spawn] Created child PID 6 for parent PID 1
[spawn] Success: child PID 6 scheduled
[heartbeat] tid=9 uptime_ms=6463 kbd_nonzero=0
[heartbeat] tid=9 uptime_ms=7465 kbd_nonzero=0
```

Periodic heartbeat output continued until the serial ended:

```text
[heartbeat] tid=9 uptime_ms=115814 kbd_nonzero=0
[heartbeat] tid=9 uptime_ms=116815 kbd_nonzero=0
[heartbeat] tid=9 uptime_ms=117820 kbd_nonzero=0
[heartbeat] tid=9 uptime_ms=118822 kbd_nonzero=0
[heartbeat] tid=9 uptime_ms=119825 kbd_nonzero=0
```

With the seam absent, nothing woke the first wait's key. The untimed wait did not return, so init remained in `waitpid`. The gate counted zero seam-absent markers, zero init-resumed markers, and zero bsshd markers. `[init] futex_handoff_oracle exited pid=` and `bsshd: listening` are absent from the red serial.

## Green: fixed self-limiting driver

After restoring the fixed tree, rebuilding userspace and the aarch64 ext2 image, the production-profile gate exited 0. `prod-profile-green-selflimiting-driver.txt` contains:

```text
[FUTEX_HANDOFF_ORACLE_DRIVER:seam_absent:probe=-110]
[init] futex_handoff_oracle exited pid=6 code=0
bsshd: listening on 0.0.0.0:2222
```

The red and green serials in this directory are the #584 B1 negative control's baseline.
