# Loopback UDP registry refusal attribution (#2144)

Registry contention is an inferred cause of the original x86-metal timeout
(`20261020T092602Z-metal`, e930247c). The absent delivery notice and concurrent
DNS activity do not identify a registry refusal for that datagram. The capture
does not confirm the precise cause of its loss.

The forcing boots establish the defect independently: a 50 ms refusal window
for bound port 54321 loses the loopback datagram before the fix
(`20261020T102952Z-metal`, a2a24045, udp_socket_test exit 9), while the fix retains
and delivers it (`20261020T103907Z-metal`, aa96394f, test exit 0). The forcing
change is absent from the PR.

## Heartbeat check

#2140 added periodic procfs counter reads and stdout tick reports to the shared
heartbeat binary. #2142 retains sampling but moves the report and its shared
atomic limiter into ARM64-only code in `kernel/src/fs/procfs/trace.rs`.
If invoked on x86, heartbeat would still incur procfs generation work, but
init's `start_liveness_service()` is ARM64-only. The original failing capture
and the forcing and fixed testing captures listed above contain no
`[heartbeat]` or `[cpu-ticks]` lines. They provide no evidence that heartbeat
added CPU, procfs, kmsg or serial contention around udp_socket_test.

The requested current-main control boot, `20261020T112132Z-metal` at ba745f71
(including #2142), exited 0 and powered off normally. Its saved verdict reports
a complete capture, 13/13 milestones and a passing userspace report. The image
build exited 0 with four Cargo jobs. udp_socket_test printed `All tests passed!`;
the tally was 261 exited and 27 nonzero. The capture contains no `[heartbeat]`
or `[cpu-ticks]` lines. This control did not reproduce the original timeout
and does not establish the cause of that earlier failure.

## Existing fixed-head evidence

At 0e5d19d8, x86-metal boots `20261020T104114Z-metal` and
`20261020T104117Z-metal` have complete captures, passing test reports and 13/13
boot-path milestones in their saved milestone verdicts. ARM64 QEMU testing
`20261020T105008Z-qemu-66541` exited 0; suite boot
`20261020T105955Z-qemu-97680` passed 15/15 suites, with memory reclaimed in
15/15 and networking 194/194. These saved records are reused for the prose
correction; it changes no executable code or test expectations.
