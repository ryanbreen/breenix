**NOT LANDED — landing gate blocked (issue 960).** At `753b4636`, ARM strict passed 1/1 and the ARM oracle passed 30/30 pipe/FIFO plus 12/12 console/tty arms. Beast x86 regression emitted TIMER_WAKE_LATENCY_ORACLE FAIL with backstops=3 and exited 1 after the operator stopped its lane-owned QEMU. The x86 oracle was not started. Both completed gate preflights and the x86 regression preflight passed 67/67 suites. No landing retry or merge was performed. Full evidence is in the round document's Landing section and `docs/planning/green-program/ipc/serials/813-c/landing-x86-boot.txt`.

Historical evidence, before this failed landing attempt:

**PR C gate set complete.** The cont2-authorized single fresh beast x86 regression attempt at `4bdbf267a87cbfe8ea4545c3f40ae75e7918f0d7` passed (`SMP_EXIT=0`, 67/67 enabled preflight suites, no panic). The original intermittent `kernel/src/task/softirq_tests.rs:228` failure remains preserved and issue 891 stays open. The cont2 evidence commit is `25882e09`.

Generic `/dev/console` and `/dev/tty` reads previously returned EAGAIN on empty input even for blocking descriptors. They now check and prepare a wait under the live input-ring guard, drop the guard before the shared wait lifecycle, and wake through the common enqueue primitive. Descriptor O_NONBLOCK, caught-signal cleanup, partial reads, and POLLIN use that same input source.

The existing blocking-I/O driver includes six console arms for both device paths. The scorer requires those twelve results on both architectures, alongside the pipe/FIFO arms and successful worker reap. A native capture-drain preflight fixture also needed a bounded file rendezvous to replace a stalled FIFO handshake; its partial/complete assertions remain.

Evidence and retained failed attempts: `docs/planning/green-program/ipc/813-PR-C-2026-09-07.md` and `docs/planning/green-program/ipc/serials/813-c/`.

Validation: ARM and beast x86 oracle matrices (42 arms each), ARM strict 3/3, beast x86 regression PASS in one cont2 attempt (original issue 891 failure retained), production ARM 1/1 last in the ARM sequence, structural suites, 21 rejected-and-restored structural mutations, and claim lint. Runtime mutation boots were not run. The pinned ARM toolchain's existing core NEON future-compatibility notice is disclosed in the round.

Issue 813 remains open: this branch does not include PR B's Unix-stream repair. No hardware keyboard delivery, canonical/EOF overhaul, PTY repair, job control, global flag-sharing repair, or SIGPIPE generation is claimed.


Production INPUT_INJECT rejection is not claimed: the retained production control probes the futex seam only. Mutation provenance is nineteen runs from 3a8a973b with PR C changes and two from 8ff71840 with the tightened ratchet. This body has an explicit file-specific claim-lint check recorded in the round.
