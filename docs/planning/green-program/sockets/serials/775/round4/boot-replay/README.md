# The boot-thread replay `kstrandd` surfaced, and the control that dates it

Adding `kstrandd` reddened the x86 TEST-profile gate on 2 of 2 boots, in a way
the production profile never showed. This directory is the three arms that
found the cause.

| arm | head | result |
|---|---|---|
| `control-round3-head/` | `3495c3f3`, the round-3 head, no `kstrandd` | `GATE: PASS` on 2 of 2 boots, 150 s each |
| `without-scheduler-fix/` | `4358fd05`, `kstrandd` alone | `GATE: FAIL`, page fault at 14 s |
| `with-scheduler-fix/` | `8da34163`, `kstrandd` plus the `block_current_for_timer` guard | `GATE: FAIL`, the same page fault at 14 s |

`without-scheduler-fix/` was produced by checking out `4358fd05`'s
`kernel/src/task/scheduler.rs` onto `8da34163` and building that.
`git diff --stat 4358fd05 8da34163` shows that file as the only source
difference between the two commits -- everything else in the range is
committed serial captures -- so the kernel it built is `4358fd05`'s.

The two failing arms are byte-similar: `Kernel page fault at 0x1e`, kernel CS,
`RSP=0xffffc90000101378`. That RSP is inside kernel stack 1 --
`0xffffc90000082000-0xffffc90000102000`, allocated in the same boot for the
idle thread (`Allocated kernel stack 1 ... Idle thread kernel stack allocated`),
which the scheduler then adopts as `init_task`/`swapper/0`. So the faulting
thread is the boot thread, on its own stack, 0xc88 below the top.

The same fault -- same address, same RSP -- is what the round-2 implementation
slot hit when it first tried a dedicated census kthread and abandoned it
without a diagnosis.

## What the capture shows

`with-scheduler-fix/serial_kernel.txt`, lines 660-670:

```
KTHREAD_STOP_AFTER_EXIT_TEST: AlreadyStopped returned correctly
=== KTHREAD STOP AFTER EXIT TEST: Completed ===
✓ Loaded 'hello_time' from test disk (177672 bytes)
✓ Loaded 'register_init_test' from test disk (177160 bytes)
✓ Loaded 'clock_gettime_test' from test disk (184608 bytes)
KTHREAD_STOP_AFTER_EXIT_TEST: AlreadyStopped returned correctly
[DISPATCH_STRAND_CENSUS:seq=3:tick=534:ms=2742:saved=0:stranded=0:...]
KTHREAD_STOP_AFTER_EXIT_TEST: AlreadyStopped returned correctly
=== KTHREAD STOP AFTER EXIT TEST: Completed ===
```

The boot thread finishes the stop-after-exit test, proceeds into
`userspace_test`'s disk loading, and then re-executes the END of the
stop-after-exit test -- twice. That is a stale context being restored: a save
that should have captured the thread at the `userspace_test` point did not
land, so the next restore replayed the last one that did. The fault follows,
in `core::sync::atomic::atomic_load`, on a byte load through a pointer read out
of a stack slot the replayed code no longer owns.

## Why `kstrandd` is what surfaced it

The dispatch breadcrumbs on COM1 are the rate. The failing boot reaches
`<K>`=32 `<1>`=27 `<I>`=0 in 14 seconds; the passing round-3 head's committed
150-second capture (`../../round3/r3-head-green/serial_user.txt`) has `<K>`=73
`<1>`=27 `<I>`=28. `<1>` is the branch that restores the idle thread's SAVED
context instead of sending it to `idle_loop`, and it is the boot-time-only
special case the code's own comment warns about. `kstrandd` wakes once a
second from the moment it is spawned, so it forces that branch tens of times
during the pre-userspace init phase, where the round-3 head took it rarely.

## The producer that made the save a no-op

`Scheduler::block_current_for_io_publish` set `blocked_in_syscall = true`
unconditionally, exactly as `block_current_for_timer` did. On x86 that flag
routes the context save into `save_kernel_context_with_guard`, which writes the
registers into `process.main_thread` -- and the boot thread has no process, so
the save leaves the registers unwritten and returns normally. The boot thread reaches that
producer during init: `Completion::wait_timeout_uninterruptible()` backs the
ext2 root read. A preemption while the flag is stale-set silently drops the boot
thread's context, and the next `<1>` replays the last save that landed.

Both producers now set the flag from `thread.owner_pid.is_some()`.
<!-- claim-lint:ok: the 3 arms above are the 3 subdirectories here, each with
     its own gate transcript. -->

## The result

With both producers guarded, the same gate is `GATE: PASS` on 2 of 2 boots,
150 s each: `../gate-green/boot{1,2}/`. The dispatch breadcrumbs move with it --
`<K>`=197 `<I>`=137 `<1>`=32 on boot 1, against `<K>`=32 `<I>`=0 `<1>`=27 on the
failing arm -- so the boot thread now reaches `idle_loop` instead of being
restored, over and over, from a boot-time saved context.
<!-- claim-lint:ok: the 2 green boots and the 2 failing ones are the four
     transcripts under serials/775/round4/. -->
