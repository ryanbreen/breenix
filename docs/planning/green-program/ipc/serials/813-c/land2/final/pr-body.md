Generic `/dev/console` and `/dev/tty` reads now wait for input readiness on blocking descriptors. The syscall route preserves O_NONBLOCK, uses the guarded input predicate and shared prepared-wait lifecycle, and cleans up after caught signals. POLLIN observes the same input ring.

Merged-tip landing gates at `87cf3de694cea98806e66d9352d1cb56329b81b1` (main `fbb1cd3b` already included):

- `bash scripts/run-structure-tests.sh`: exit 0, 69/69 suites.
- `bash docker/qemu/run-aarch64-boot-test-strict.sh 1`: exit 0, 1/1.
- `bash docker/qemu/run-x86-boot-tests.sh`: exit 0; TIMER_WAKE_LATENCY_ORACLE PASS, backstops=0, overrun_ms=44; SOFTIRQ_DEFERRAL_ORACLE verdict=ok. Beast command-launch load 3.76; recorded QEMU-start load 13.60.
- `bash docker/qemu/run-blocking-io-oracle-gate.sh --arch x86_64`: exit 0, 30/30 pipe/FIFO and 12/12 console/tty arms, exact byte tallies and worker reaped. Beast command-launch load 0.90; its timer record also PASS with backstops=0.

Each of the three guest gates passed its 69/69 enabled structure preflight. This final attempt used 0 guest retries and 0 structure timeout retries. The changed kernel files pass rustfmt; project compiler diagnostics count is 0. The accepted pinned-nightly ARM core notice is retained.

Evidence, exact oracle records, revisions, loads and claim checks: `docs/planning/green-program/ipc/813-PR-C-2026-09-07.md`, final Landing section, and `docs/planning/green-program/ipc/serials/813-c/land2/final/`.

The PR chain is A 933, A2 939, B 966, C 958. The merged source and ratchet cover 4/4 issue 813 families with 0 prohibited blocking EAGAIN exits. The final ratchet also audits the Console/Tty syscall result route. Issue closure follows confirmation of this PR's merge.

Historical failed landing evidence retained: at `753b4636`, ARM strict passed 1/1 and ARM oracle passed 30/30 + 12/12, but the beast regression emitted timer-wake FAIL with backstops=3 and exited 1 after the operator stopped its lane-owned QEMU. The x86 oracle was not started. Original transcript: `docs/planning/green-program/ipc/serials/813-c/landing-x86-boot.txt`. The original ksoftirqd failure and the subsequent pre-fix timer reproduction also remain in the round document. These failed attempts are not erased or counted as passing.

Correction dfe14edf makes the timer oracle coordinator a schedulable kthread with forced dispatch during peer release; the bound and backstop rejection are unchanged. Two historical dfe14edf x86 reruns passed with backstops=0 (QEMU-start loads 13.28 and 3.12). PR 967's softirq correction is included in the landing tip.

Not claimed: fresh ARM oracle or production boots at 87cf3de6 (their 9ee19079 results remain historical), runtime mutation boots, separate Unix runtime-oracle execution, canonical input/VMIN/VTIME, hardware keyboard delivery or PTY repair.
