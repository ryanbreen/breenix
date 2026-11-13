# Issue 2823: unanswered xHCI replay

## Retained evidence

Read issue #2823, including its comment about #2827, ARM ledger entries
445-448 (the coordinator ruling is currently entry 448), and E23 entry 456.
Copies of both retained boot directories were read from beast through
`sudo -n incus exec breenix-metal` into `/tmp/i2823-captures/`.

`20261113T015747Z-metal/serial.txt:1288` records Taken, worker 20
BlockedOnIO on CPU 0, pending=true, waiter=20, LEDs=false, IRQ count 154.
Its command deadline is 170226172433 ns. The request age is 8368734204 ns.
The case times out at line 1291; the next request gets EBUSY at line 1319.
There is no command-timeout snapshot in this serial capture.
`20261109T125150Z-metal/serial.txt:1469,1497` has the same two failed
cases before the killable mailbox change; it does not have worker-state evidence.
These captures do not identify the command or the IRQ-to-wake progress.

## Code reach (origin/main 1da274049402)

Line references in this section name the unmodified base tree.

* `input/control.rs:46-69,92-100` validates the request and calls the registered
  driver without a descriptor lock. `xhci/keyboard.rs:857-876` routes the
  request through the mailbox and answers after its worker transaction returns.
* `xhci/suite_replay.rs:64-73` reserves, prepares and posts the mailbox, then
  wakes through `request`. `xhci/mod.rs:131-137` stores the request flag before
  reading the waiter. `spec.rs:388-405` puts work and waiter publication in one
  SeqCst order. `mod.rs:952-970` checks work before and after publishing the
  blocked state. A post before publication is seen by a predicate; a post after
  publication can wake the published waiter. This is not evidence of a simple
  unguarded post-versus-block gap.
* POSTED can remain untaken while the worker is in enumeration, console/replug,
  LED or another synchronous command wait (`keyboard.rs:51-112`), or if its
  scheduler wake is not delivered. TAKEN excludes an untaken submit in the
  newer capture: `suite_replay.rs:40-42` changes POSTED to TAKEN at worker entry.
* For a keyboard transaction, `keyboard.rs:878-900` executes press and release.
  Each calls `replay_key:149-204`: Stop Endpoint at 166, then Set Dequeue at 197.
  `mod.rs:1432-1434` waits for Set Dequeue's command completion too. Report
  delivery can also call synchronous LED control (`keyboard.rs:507-572`,
  `117-127,469-484`). Pointer replay has the corresponding Stop/Set Dequeue
  waits (`pointer.rs:442-506`). These are the replay's I/O blocking sites.
* `mod.rs:1005-1021` selects a matching completion by type and TRB address,
  then waits for MSI work with a fixed three-second deadline. The serve loop,
  synchronous waits and dispatch run on one controller owner; there is no
  second concurrent ring consumer. `service:892-938` queues command/transfer
  events; `keyboard.rs:207-240` dispatches after synchronous waits return.
  A missing/mismatched completion can wait until the deadline; unrelated
  completions remain queued during the synchronous wait.
* `mod.rs:186-201` acknowledges USBSTS and IMAN, publishes WORK, and raises
  IrqPoll when a waiter exists. `deferred_wake:207-211` wakes through the
  scheduler owner path. `task/scheduler.rs:9249-9255,9262-9269` and
  `scheduler/rq.rs:1441-1461` retain a busy-owner wake. A pending bit is not
  itself a scheduler wake. `per_cpu.rs:779-796,1092-1094` defers softirqs
  while preemption/IRQ/BH context excludes dispatch. Delay between publication
  and deferred callback can therefore leave pending=true and BlockedOnIO.
* `wait_work:970-984` watches scheduler state while parked, rather than work;
  the scheduler wake or deadline must make it runnable. Timed I/O publication
  arms the timer (`scheduler/rq.rs:2089-2117`); expiry changes the state and
  enqueues it (`1019-1101`). A stalled expiry/dispatch is a separate candidate
  from a controller that has not delivered its completion.
* `service:926-933` advances ERDP and clears EHB even on an empty drain. It
  does not re-arm a transient per-command interrupt. A pending event with
  matching cycle but EHB uncleared, an undelivered MSI, or a command without an
  event requires hardware state to distinguish. `completion_timeout:1024-1048`
  prints only after the wait returns false, so it cannot explain a wait that
  has not returned or repeatedly reaches completion only at its deadline.
* `suite_replay.rs:98-103` preserves worker custody when the caller dies;
  `44-58` releases the abandoned mailbox when the worker answers. Thus EBUSY
  after a killed TAKEN transaction is expected until the answer. No new
  timeout or DMA release is proposed.

## Diagnostic question

Does replay progress command-by-command only on its three-second deadlines,
with MSI work pending but its deferred wake delayed, or is one command's
completion actually absent/unconsumed? The eight command slots retain wanted
TRB/type/opcode, begin/deadline/finish times, IRQ count and completion code.
The summary records the last IRQ waiter/CPU/preempt state, deferred callbacks,
and the worker's last blocking phase/CPU/preempt state. The cold hardware
snapshot reads USBSTS, IMAN, ERDP and the awaited event-ring entry/cycle.
These are best-effort concurrent snapshots, not an atomic controller dump.

The production host check `tests/xhci_wait_evidence.rs` was written first
(failed because the evidence module was absent), then passed with the module.
It checks the diagnostic distinction and eight-slot bound. It is not a
reproduction of the unanswered-worker defect. No behavioral fix is present.
Only the existing killed-replay path emits these records. The counters and
slots stay enabled, without a per-boot toggle. IRQ updates use atomics and
lock-free CPU/preemption reads; no IRQ logging or allocation was added.

## Validation before the diagnostic boot

The mailbox host fixture's six tests share mutable globals. Running them in
one process in alphabetical order makes its initial-disabled assertion fail
when another test has enabled the latch. The mailbox behavior check passes
when invoked as its own process. This pre-existing fixture-order problem was
not repaired or used as evidence against the xHCI driver.
