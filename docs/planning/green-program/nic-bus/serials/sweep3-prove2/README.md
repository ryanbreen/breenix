# Sweep-3 fix round 2 -- PROVE round evidence

Branch `chore/sweep-3` @ `a6679e7c` (the fix-round head, unchanged by this
prove round's own commit -- this round only ADDS a new regression test and
evidence). Read `fix2-notes.md` and the original `review.md` in full before
starting. Every claim below was executed by this prove pass, not re-asserted
from the fix round's own notes.

## Verdict: NOT YET LANDABLE -- one new, severe, blocking finding

**B4-AARCH64-REGRESSION.md**: B4's `is_valid_user_range` redo, while
correctly closing the x86 kernel-address-disclosure hole it was built for
(confirmed, x86 leg below), simultaneously breaks EVERY aarch64 userspace
process's first buffered `write()` syscall -- `init` (PID 1) itself panics
on its first `print!()` with EFAULT and dies, so nothing downstream of the
kernel-internal boot-test suite ever runs. Isolated with an A/B control
(same borrowed userspace artifacts, pre-B4 tree boots clean, current-HEAD
tree does not) that rules out environment reconstruction as the cause. This
was never caught because fix2-notes.md's own aarch64 verification was a
`cargo build` compile check only -- never an actual boot under the
`boot_tests`/`full` feature profile this gate exists to protect.

## B4 (#729) -- x86 leg confirmed; aarch64 leg regressed (see above)

- `b4-aarch64-mutation-falsification.txt`: the compile-time proof block in
  `kernel/src/memory/layout.rs` is load-bearing on both arches -- forcing
  `is_valid_user_range` to `return true` unconditionally is caught at
  `cargo build` time (not a passing build), restored and reconfirmed clean.
- x86: the same compile-time proof executes on every x86 build (beast,
  `docker/qemu/run-x86-gate.sh 1 full`, GATE: PASS below) -- a kernel PIE
  base and the kernel heap start are refused, a representative user
  code/data address is accepted, per the two x86-specific `const _:`
  assertions plus the arch-generic heap/user-address ones.
- `B4-AARCH64-REGRESSION.md` + `b4-aarch64-a6679e7c-first-crash-20260901.txt`
  + `b4-aarch64-a6679e7c-second-crash-repro-20260901.txt` (reproduced 2/2,
  identical failure signature) + `b4-aarch64-preB4-control-clean-boot.txt`
  (A/B control, clean).

## B1-B3 (#739) -- confirmed, both legs

- `b1-b3-unit-test-mutation-falsification.txt`: ran the committed
  `739-gdb-chat-fix/unit-test-resync-symbols.txt` myself (GREEN), then
  reintroduced the B1 bug in a scratch copy of `gdb_chat.py` and reran the
  identical test (RED, fails exactly where B1 predicts). Not vacuous.
- `b1-b3-live-forced-mismatch-beast.txt`: this beast container has only ever
  been observed to pick `0x10000000000` (the fix round's own live-two-boot
  and six-probe-boot evidence agree), so the mismatch path never occurs
  naturally here -- forced it by temporarily setting the class-level guess
  to the other historically-observed value (`0x8000000000`) and driving a
  REAL GDB session end to end. `remove-symbol-file` genuinely issued against
  the stale guess-based address; the reload's `add-symbol-file` used the
  real discovered base (read straight off the boot's own serial line,
  computed and verified independently); `info symbol $pc` resolved to a
  real kernel function post-resync. gdb_chat.py restored to its committed
  state afterward (`git diff --stat` empty).

## M1 (#724) -- confirmed, new regression test added, mutation-verified

- `m1-tcp-dup-listener-test.md`: added
  `userspace/programs/src/tcp_dup_listener_test.rs` (bind+listen, dup, close
  original, prove the survivor still accepts connections, close the
  survivor, prove the port is genuinely free again). GREEN on the full x86
  gate (`exited=20`, up from 19, `nonzero=0`); the test's own
  `TCP_DUP_LISTENER_TEST_PASSED` marker and both step-level PASS lines
  appear in the gate's serial log. RED when the exact inc-side fix this test
  targets is reverted (`TCP_DUP_LISTENER_TEST_FAILED`, exact predicted
  message). Not vacuous.

## Regression

- `docker/qemu/run-x86-gate.sh 1 full` on beast: **GATE: PASS**
  (`exited=20 expected>=10 nonzero=0 allowlist=0`, build clean 0 warnings,
  16s build / 150s boot).
- `docker/qemu/run-x86-boot-tests.sh 5` on beast: see
  `x86-boot-tests-5-run-20260901.txt` (this gate is a heavy frame/page-table
  custody stress suite under TCG on this run -- slow but not the same code
  paths B4/B1-B3/M1 touch).
- `./docker/qemu/run-aarch64-full-test.sh --boot-tests-only` (local Mac,
  QEMU quiet-waited first): **FAILS** at `a6679e7c` -- see the B4 aarch64
  finding above. This is the honest, unattributed, blocking regression this
  prove round exists to surface.
- Host structural suites: see `host-suites-20260901.txt`.

## Ledger

Appended one note to
`/Users/wrb/.claude/workflow-ledgers/breenix-r22-minimal-2026-08-03.jsonl`
flagging the aarch64 finding as blocking.
