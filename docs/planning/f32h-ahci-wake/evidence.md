# F32h AHCI Wake Evidence

## Capture Method

F32h used temporary in-memory tracing probes only. No serial breadcrumbs were
added near spawn, AHCI IRQ handling, completion wake, or scheduler wake paths.
The probes wrote `F32H_*` records into the existing per-CPU trace buffers and
were dumped only after a soft-lockup detector fired. The temporary probes were
removed before committing this document.

The all-CPU soft-lockup trigger used for these captures was also temporary. Run
1 reproduced the original no-progress spawn stall at `[spawn] path='/bin/bwm'`,
but CPU0 did not emit a lockup dump within 120s. That is itself useful evidence:
the old CPU0-only dump path misses the same signature where CPU0 stops making
timer progress.

Raw artifacts are under:

- `.factory-runs/f32h-ahci-wake-20260419_050419/parallels-run1/serial.log`
- `.factory-runs/f32h-ahci-wake-20260419_050419/parallels-run3/serial.log`
- `.factory-runs/f32h-ahci-wake-20260419_050419/parallels-run4/serial.log`

## Summary Finding

The two trace-backed failing Parallels runs do not show an unconsumed ISR wake
ring entry. For the observed ext2 reads, the chain completes:

`wait blocks -> AHCI IRQ fires -> command completion sees waiter tid 10 ->
isr_unblock_for_io(10) pushes the ISR buffer -> scheduler drains it -> wake_io
sees BlockedOnIO -> task is made runnable/enqueued`.

The break seen in these captures is after wake/enqueue, in scheduler or timer
progress. This does not prove the deferred ring is correct in every interleaving,
but it disproves "ring write not consumed" for the captured failures.

## Run 3: Failure After bsshd, Before Normal bounce Startup

Serial progression:

- `serial.log:372`: `[spawn] path='/bin/bsshd'`
- `serial.log:381`: bsshd process creation succeeds
- `serial.log:386`: `[init] bsshd started (PID 4)`
- `serial.log:389`: soft lockup dump starts before normal bounce startup

At the first dump:

- Ready queue length is 0 (`serial.log:392`).
- CPU0 is idle-current with init as previous: `CPU 0: current=0 previous=10`
  (`serial.log:396`).
- Init, tid 10, is not stranded as BlockedOnIO at the dump. In the first dump it
  is `state=T` (BlockedOnTimer) (`serial.log:415`); later dumps show tid 10 as
  `state=R` (`serial.log:920`, `serial.log:5665`, `serial.log:10400`).
- The global tick counter is stuck at 147 while aggregate timer IRQs continue:
  `TIMER_TICK_TOTAL: 7473`, later `129474`, `287417`, `407140`, while
  `Global ticks: 147` remains unchanged (`serial.log:5608-5615`,
  `10353-10360`, `19818-19825`, `38738-38745`).

Representative ext2/AHCI completion chain for command token 794:

```text
serial.log:446  CPU0 ts=63702363 F32H_WAIT_ENTRY expected_token=794
serial.log:447  CPU0 ts=63702363 F32H_WAIT_WAITER_SET tid=10
serial.log:448  CPU0 ts=63702388 F32H_WAIT_BLOCK tid=10
serial.log:461  CPU0 ts=63702923 F32H_AHCI_IRQ_HBA_IS hba_is=2
serial.log:462  CPU0 ts=63703054 F32H_AHCI_PORT_IS port_is=1 flags=0x1
serial.log:463  CPU0 ts=63703054 F32H_AHCI_PORT_CI port_ci=0 flags=0x1
serial.log:464  CPU0 ts=63703054 F32H_AHCI_ACTIVE active_mask=1 flags=0x1
serial.log:465  CPU0 ts=63703168 F32H_AHCI_COMPLETED completed_slots=1 flags=0x1
serial.log:466  CPU0 ts=63703168 F32H_AHCI_CMD cmd_num=794 flags=0x1
serial.log:467  CPU0 ts=63703168 F32H_AHCI_WAITER waiter_tid=10 flags=0x1
serial.log:468  CPU0 ts=63703231 F32H_AHCI_COMPLETE_CALL cmd_num=794 flags=0x1
serial.log:469  CPU0 ts=63703232 F32H_COMPLETE_DONE token=794
serial.log:470  CPU0 ts=63703232 F32H_COMPLETE_WAITER tid=10
serial.log:471  CPU0 ts=63703232 F32H_ISR_UNBLOCK_CALL tid=10
serial.log:472  CPU0 ts=63703232 F32H_ISR_BUF_PUSH tid=10
serial.log:473  CPU0 ts=63703413 F32H_ISR_DRAIN_TID src_cpu<<24|tid=10
serial.log:474  CPU0 ts=63703426 F32H_WAKE_IO_BEFORE state<<24|tid=100663306
serial.log:475  CPU0 ts=63703430 F32H_WAKE_IO_AFTER target<<24|current<<16|tid=16711690
```

Decode:

- `state<<24|tid=100663306` is `0x0600000a`: state 6 is BlockedOnIO, tid 10.
- `target<<24|current<<16|tid=16711690` is `0x00ff000a`: target CPU 0,
  current CPU sentinel 0xff, tid 10. The task was enqueued for CPU0 and was not
  current on any CPU at that wake.
- The ISR buffer push-to-drain latency for this token is
  `63703413 - 63703232 = 181` trace timestamp ticks.

The run also captured fast completions where the IRQ beat waiter installation.
For token 797, AHCI command completion appears before the wait entry:

```text
serial.log:580  CPU0 ts=63709522 F32H_AHCI_CMD cmd_num=797 flags=0x1
serial.log:581  CPU0 ts=63709522 F32H_AHCI_WAITER waiter_tid=0 flags=0x1
serial.log:583  CPU0 ts=63709929 F32H_COMPLETE_DONE token=797
serial.log:584  CPU0 ts=63709929 F32H_COMPLETE_WAITER tid=0
serial.log:585  CPU0 ts=63710186 F32H_WAIT_ENTRY expected_token=797
```

That is a valid fast-path case, not the stall: the waiter is absent because the
completion finished before the task committed to blocking.

Ring snapshot at the first dump:

```text
serial.log:1957 CPU0 F32H_ISR_RING_DEPTH cpu<<16|depth=0
serial.log:1958 CPU0 F32H_ISR_RING_DEPTH cpu<<16|depth=65536 flags=0x1
serial.log:1959 CPU0 F32H_ISR_RING_DEPTH cpu<<16|depth=131072 flags=0x2
serial.log:1960 CPU0 F32H_ISR_RING_DEPTH cpu<<16|depth=196608 flags=0x3
serial.log:1961 CPU0 F32H_ISR_RING_DEPTH cpu<<16|depth=262144 flags=0x4
serial.log:1962 CPU0 F32H_ISR_RING_DEPTH cpu<<16|depth=327680 flags=0x5
serial.log:1963 CPU0 F32H_ISR_RING_DEPTH cpu<<16|depth=393216 flags=0x6
serial.log:1964 CPU0 F32H_ISR_RING_DEPTH cpu<<16|depth=458752 flags=0x7
```

The payload is `(cpu << 16) | depth`; all depths are 0. The current structure is
a per-CPU slot buffer rather than a head/tail ring, so "head/tail" maps to
"pending slot count" for this implementation.

## Run 4: Failure At telnetd Spawn

Serial progression:

- `serial.log:340`: `[spawn] path='/bin/bwm'`
- `serial.log:349`: bwm process creation succeeds
- `serial.log:360`: `[spawn] path='/sbin/telnetd'`
- `serial.log:363`: soft lockup dump starts before telnetd process creation
  finishes

At the first dump:

- Ready queue length is 0 (`serial.log:366`).
- CPU0 is idle-current with init as previous: `CPU 0: current=0 previous=10`
  (`serial.log:370`).
- Init, tid 10, is `state=R` (`serial.log:389`).
- Global ticks are stuck at 72 while aggregate timer IRQs continue:
  `TIMER_TICK_TOTAL: 4272` then `82131`, while `Global ticks: 72` remains
  unchanged (`serial.log:4001-4008`, `11318-11325`).

Representative chain for command token 417:

```text
serial.log:439  CPU0 ts=57284348 F32H_WAIT_ENTRY expected_token=417
serial.log:440  CPU0 ts=57284349 F32H_WAIT_WAITER_SET tid=10
serial.log:441  CPU0 ts=57284381 F32H_WAIT_BLOCK tid=10
serial.log:454  CPU0 ts=57284615 F32H_AHCI_IRQ_HBA_IS hba_is=2
serial.log:455  CPU0 ts=57284743 F32H_AHCI_PORT_IS port_is=1 flags=0x1
serial.log:456  CPU0 ts=57284743 F32H_AHCI_PORT_CI port_ci=0 flags=0x1
serial.log:457  CPU0 ts=57284743 F32H_AHCI_ACTIVE active_mask=1 flags=0x1
serial.log:458  CPU0 ts=57284837 F32H_AHCI_COMPLETED completed_slots=1 flags=0x1
serial.log:459  CPU0 ts=57284837 F32H_AHCI_CMD cmd_num=417 flags=0x1
serial.log:460  CPU0 ts=57284837 F32H_AHCI_WAITER waiter_tid=10 flags=0x1
serial.log:461  CPU0 ts=57284897 F32H_AHCI_COMPLETE_CALL cmd_num=417 flags=0x1
serial.log:462  CPU0 ts=57284897 F32H_COMPLETE_DONE token=417
serial.log:463  CPU0 ts=57284897 F32H_COMPLETE_WAITER tid=10
serial.log:464  CPU0 ts=57284897 F32H_ISR_UNBLOCK_CALL tid=10
serial.log:465  CPU0 ts=57284897 F32H_ISR_BUF_PUSH tid=10
serial.log:466  CPU0 ts=57285048 F32H_ISR_DRAIN_TID src_cpu<<24|tid=10
serial.log:467  CPU0 ts=57285061 F32H_WAKE_IO_BEFORE state<<24|tid=100663306
serial.log:468  CPU0 ts=57285062 F32H_WAKE_IO_AFTER target<<24|current<<16|tid=16711690
```

Decode:

- `0x0600000a`: tid 10 was BlockedOnIO at wake.
- `0x00ff000a`: tid 10 was enqueued for CPU0 and not current.
- Push-to-drain latency is `57285048 - 57284897 = 151` trace timestamp ticks.

Ring snapshot:

```text
serial.log:3673 CPU6 F32H_ISR_RING_DEPTH cpu<<16|depth=0
serial.log:3674 CPU6 F32H_ISR_RING_DEPTH cpu<<16|depth=65536 flags=0x1
serial.log:3675 CPU6 F32H_ISR_RING_DEPTH cpu<<16|depth=131072 flags=0x2
serial.log:3676 CPU6 F32H_ISR_RING_DEPTH cpu<<16|depth=196608 flags=0x3
serial.log:3677 CPU6 F32H_ISR_RING_DEPTH cpu<<16|depth=262144 flags=0x4
serial.log:3678 CPU6 F32H_ISR_RING_DEPTH cpu<<16|depth=327680 flags=0x5
serial.log:3679 CPU6 F32H_ISR_RING_DEPTH cpu<<16|depth=393216 flags=0x6
serial.log:3680 CPU6 F32H_ISR_RING_DEPTH cpu<<16|depth=458752 flags=0x7
```

Again, all per-CPU pending depths decode to 0.

## Direct Answers

1. When `load_elf_from_ext2` blocks, the blocking thread is tid 10, the init
   task. It blocks on an AHCI per-slot completion token, not a waitqueue. The
   wake trace confirms the blocked state as BlockedOnIO (`state=6`) immediately
   before the ISR wake transition.

2. Yes. In run 3, token 794 blocks at ts 63702388 and the AHCI IRQ arrives at
   ts 63702923 with `hba_is=2`, `port_is=1`, `port_ci=0`, `active_mask=1`, and
   `completed_slots=1`. In run 4, token 417 blocks at ts 57284381 and the IRQ
   arrives at ts 57284615 with the same PxIS/PxCI completion pattern.

3. Yes. Run 3 token 794 records `F32H_AHCI_WAITER waiter_tid=10`,
   `F32H_COMPLETE_WAITER tid=10`, and `F32H_ISR_UNBLOCK_CALL tid=10`. Run 4
   token 417 records the same sequence for tid 10.

4. Yes. Run 3 drains on CPU0 181 trace timestamp ticks after the push. Run 4
   drains on CPU0 151 trace timestamp ticks after the push. Other later drains
   also appear on CPUs 1, 2, 5, 6, and 7, which confirms cross-CPU drain is
   active, but the representative failing-spawn chains above drain on CPU0.

5. At the first trace dump in both trace-backed failures, all per-CPU pending
   depths are 0. This implementation has fixed per-CPU slots rather than
   head/tail indices.

6. CPU0 is idle-current at the dumps (`current=0`) with init as its previous
   thread (`previous=10`). In run 4, init is Ready at the dump. In run 3, the
   first dump catches init BlockedOnTimer, and later dumps catch it Ready.

7. The aggregate timer interrupt counter continues advancing on other CPUs, but
   the global tick counter stops. Run 3 keeps `Global ticks: 147` while
   `TIMER_TICK_TOTAL` rises from 7473 to 407140. Run 4 keeps `Global ticks: 72`
   while `TIMER_TICK_TOTAL` rises from 4272 to 82131. This matches the F32c
   signature class: CPU0/global tick progress stops while other CPUs still take
   timer interrupts.

## Break In Chain

For both trace-backed failures, the chain does not break at:

- AHCI IRQ delivery
- PxIS/PxCI completion detection
- completion waiter lookup
- `isr_unblock_for_io(tid)`
- ISR buffer push
- ISR buffer drain
- `BlockedOnIO -> Ready` transition

The captured break is after the task is made runnable/enqueued, with CPU0/global
tick progress stalled and ready queue state inconsistent with expected forward
progress. F32i should therefore avoid assuming an unconsumed ISR ring entry
unless a future capture catches non-zero pending slot depth or a missing drain.
