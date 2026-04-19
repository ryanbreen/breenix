# F32m Tick-Audit Diagnosis

## Root Cause

`[timer] cpu0 ticks=...` is not an end-of-run audit. It is a periodic
interrupt-context serial breadcrumb emitted only by CPU 0 when
`TIMER_TICK_COUNT[0] % 5000 == 0`.

The emission uses `serial_aarch64::raw_serial_str()`, which writes bytes
directly to the UART data register. That path does not acquire the serial lock
and does not wait for transmit FIFO space. On Parallels, normal locked serial
output from other CPUs can interleave with or fill the UART while the timer
breadcrumb writes. When that happens, the complete parser-required substring is
lost even though the guest keeps booting, bsshd and bounce run, frames render,
and screenshots pass.

The fix should make the reporting path reliable without changing the parser:
the timer audit should become pending-until-printed and should use a
nonblocking serial-lock emission path that waits for FIFO capacity only after
successfully acquiring the serial lock. That keeps the timer IRQ from blocking
on a contended serial lock and keeps the reported value as the real CPU0 tick
counter.

## Emission Path

- Producer:
  `kernel/src/arch_impl/aarch64/timer_interrupt.rs`
- Counter:
  `TIMER_TICK_COUNT[0]`
- Trigger:
  CPU 0 timer interrupt, every exact 5000 CPU0 timer ticks.
- Output path:
  `serial_aarch64::raw_serial_str(b"[timer] cpu0 ticks=")`, manual decimal
  formatting, then `raw_serial_str(b"\n")`.
- Parser:
  F32k's validation script parses
  `r"\[timer\] cpu0 ticks=(\d+)"` and requires `max_cpu0_ticks > 1000`.

There is no unconditional shutdown/end-of-run dump for this evidence line.
If the raw periodic line is dropped or split, the run has no second source of
CPU0 tick evidence.

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

## Ruled Out

- Feature gating: the producer is not behind a feature flag on this branch.
- Parser mismatch: the parser's expected string exactly matches the producer's
  prefix when the line survives intact.
- Test-gate weakness: the gate is correct to require the line; the missing
  reliability is in the producer.

## Fix Direction

Keep the cadence at 5000 CPU0 ticks. At 1000 Hz this is every 5 seconds, so a
healthy 120-second run has many opportunities to emit the line. Instead of a
single raw UART attempt at the exact modulo tick, mark the audit count pending
and retry on later CPU0 timer interrupts until a complete locked serial write
succeeds.

