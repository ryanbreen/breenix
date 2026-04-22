# F34 Phase 1 Evidence

Phase 1 required reproducing CPU0 virtual timer degradation on current main
(`5780377f`) before root-cause or fix work. I could not reproduce the supplied
signature in this environment.

## Baseline Main Run

Command:

```bash
./run.sh --parallels --clean --test 120
```

Artifact: `logs/f34/baseline-main/serial.log` (ignored run artifact, not
committed).

Observed:

- bsshd started and listened on `0.0.0.0:2222`.
- bounce started and created a 400x300 window.
- CPU0 timer heartbeat continued through `cpu0 ticks=90000`.
- VirGL compositor reached frame `23500`.
- No `AHCI TIMEOUT`, no panic, no `[TRACE]` dump.

This contradicts the expected target signature, where CPU0 timer delivery
degrades shortly after tick 255 and AHCI times out during bounce ELF loading.

## Temporary Instrumented Run

Temporary trace-framework instrumentation was added at the requested sites:

- `schedule_from_kernel` entry
- `wait_timeout` entry
- `inline_schedule_trampoline` stages
- ret-based idle dispatch
- idle-loop iteration and WFI entry/exit
- `block_current_for_io_with_timeout` deadline recording

The instrumentation built clean for aarch64 and x86_64, but no failure trace was
captured. It was removed before this exit commit because it was high-volume
diagnostic code in hot paths and did not produce the required evidence.

Command:

```bash
./run.sh --parallels --test 120
```

Artifact: `logs/f34/phase1-instrumented-run1/serial.log` (ignored run artifact).

Observed:

- bsshd started and listened on `0.0.0.0:2222`.
- bounce started and created a 400x300 window.
- VirGL compositor reached frame `24000`.
- No `AHCI TIMEOUT`, no panic, no `[TRACE]` dump.

The periodic `cpu0 ticks=` heartbeat did not appear in this captured serial log,
but the system continued rendering and did not fail. Without a trace dump or
timeout, this is insufficient evidence for the requested degradation signature.

## Live Instrumented Run

Command:

```bash
./run.sh --parallels --no-build
```

Artifact: `logs/f34/live-instrumented-run2/serial.log` (ignored run artifact).

Observed:

- bsshd started and listened on `0.0.0.0:2222`.
- bounce started and created a 400x300 window.
- CPU0 timer heartbeat continued through `cpu0 ticks=60000`.
- VirGL compositor reached frame `15000`.
- No `AHCI TIMEOUT`, no panic, no `[TRACE]` dump.

I also attempted host TCP probes against `10.211.55.100` ports `2222`, `23`, and
`2323` so that `/proc/trace/buffer` could be read from inside a successful boot.
Those probes did not connect from the host, despite the guest serial showing
bsshd and telnetd listening.

## Conclusion

Phase 1 is blocked: current `5780377f` did not reproduce the CPU0 vtimer
degradation signature in three autonomous Parallels boots. Because the required
failure trace was not captured, there is no honest basis for Phase 3 root-cause
claims or a Phase 4 Linux-parity fix in this run.
