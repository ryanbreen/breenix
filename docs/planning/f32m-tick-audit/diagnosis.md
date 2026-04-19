# F32m Tick-Audit Diagnosis

## Root Cause

`[timer] cpu0 ticks=...` is not an end-of-run audit. It is a periodic
interrupt-context serial breadcrumb emitted only by CPU 0 when
`TIMER_TICK_COUNT[0] % 5000 == 0`.

That makes the factory evidence line nondeterministic for two separate reasons:

1. CPU0 can fail to reach the 5000-tick threshold in an otherwise healthy
   Parallels run. The F32m reproduction `normal-1.serial.log` reached bsshd,
   bounce, visible cursor, no AHCI timeout, and `Frame #22000`, but CPU0 only
   emitted the early raw `T1..T0` breadcrumbs and never produced a 5000-tick
   audit line.
2. When CPU0 does reach the threshold, the old emission uses
   `serial_aarch64::raw_serial_str()`, which writes bytes directly to the UART
   data register. That path does not acquire the serial lock and does not wait
   for transmit FIFO space. On Parallels, normal locked serial output from other
   CPUs can interleave with or fill the UART while the timer breadcrumb writes.
   When that happens, the complete parser-required substring is lost.

The fix should make the reporting path reliable without changing the parser:
CPU0 should remain the primary emitter, but CPU1 should take over only when CPU0
is stuck at the known early breadcrumb range (`<=10` ticks). The audit should
remain pending until printed and use a nonblocking serial-lock emission path
that waits for FIFO capacity only after successfully acquiring the serial lock.
This preserves the parser contract and keeps the line tied to real timer
interrupt progress instead of a shutdown hook or a test fallback.

## Emission Path

- Producer:
  `kernel/src/arch_impl/aarch64/timer_interrupt.rs`
- Counter:
  Before F32m, `TIMER_TICK_COUNT[0]` only. After F32m, CPU0 remains primary; if
  CPU0 is stuck at `<=10` ticks, CPU1 claims the legacy line from
  `TIMER_TICK_COUNT[1]`.
- Trigger:
  CPU 0 timer interrupt, every exact 5000 CPU0 timer ticks.
- Output path:
  `serial_aarch64::raw_serial_str(b"[timer] cpu0 ticks=")`, manual decimal
  formatting, then `raw_serial_str(b"\n")`.
- Parser:
  F32k's validation script parses
  `r"\[timer\] cpu0 ticks=(\d+)"` and requires `max_cpu0_ticks > 1000`.

There is no unconditional shutdown/end-of-run dump for this evidence line.
If CPU0 does not reach the exact threshold, or if the raw periodic line is
dropped or split, the run has no second source of tick evidence.

## Failed-run Evidence

F32k validation:

- Passing `normal-1.serial.log`: 16 complete tick-audit lines,
  `5000..80000`.
- Failing `normal-2.serial.log`: 0 complete tick-audit lines while the log
  still reaches bsshd, bounce, strict-render PASS, visible cursor, and
  `Frame #20000`.
- The failed log shows early raw timer breadcrumbs interleaved into unrelated
  boot lines, which demonstrates that raw interrupt-context UART bytes are not
  serialized with normal serial output.

F32j validation:

- Passing run 1: 17 complete tick-audit lines, `5000..85000`.
- Failing run 3: 0 complete tick-audit lines while frames continue through
  `Frame #18500`, strict render PASS, and no AHCI timeout.
- The failed run also contains interleaved raw `Tn` bytes in normal boot lines.

F32m reproduction:

- `wait-stress-2.serial.log`: CPU0 did reach audit intervals; the fixed locked
  pending path produced complete lines at `5000..75000`.
- `normal-1.serial.log`: the system reached `[init] bsshd started`, `[init]
  bounce started`, `Frame #22000`, visible cursor, and no AHCI timeout, but
  emitted zero complete audit lines. The only CPU0 timer breadcrumbs were the
  first ten raw `Tn` markers, so the CPU0-only 5000-tick trigger was not a
  deterministic evidence source.

## Ruled Out

- Feature gating: the producer is not behind a feature flag on this branch.
- Parser mismatch: the parser's expected string exactly matches the producer's
  prefix when the line survives intact.
- Test-gate weakness: the gate is correct to require a timer-progress line; the
  missing reliability is in the producer.

## Fix Direction

Keep the cadence at 5000 per-CPU timer ticks. At 1000 Hz this is every 5
seconds, so a healthy 120-second run has many opportunities to emit the line.
Instead of a CPU0-only raw UART attempt at the exact modulo tick, CPU0 claims
normal intervals and CPU1 claims intervals only when CPU0 is stuck in the
known early-timer range. Mark the audit count pending and retry on later
timer interrupts until a complete locked serial write succeeds.
