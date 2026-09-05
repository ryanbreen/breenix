# Breenix Runs

SwiftPM command-line tooling for preserving Breenix gate evidence and printing a host-facts trace for ARM gate runs.

## Build

```bash
swift build
swift test
```

`make app` is intentionally a stub in PR-1. SwiftUI app bundling lands in PR-6.

## CLI

PR-1 through PR-3 implement:

```bash
breenix-runs run arm [strict|prod|testing] [--boots N] [--tag T] [--no-store]
breenix-runs show <run-id|latest|latest-fail> [--subsystems] [--messages] [--traces]
breenix-runs facts <run-id|latest> [--json]
```

Examples:

```bash
swift run breenix-runs run arm strict --boots 20 --tag before-scheduler-change
swift run breenix-runs run arm --boots 1 --no-store
swift run breenix-runs show latest
swift run breenix-runs show latest-fail --subsystems
swift run breenix-runs show <run-id> --messages --traces
swift run breenix-runs facts latest
swift run breenix-runs facts latest --json
```

`run arm` drives the existing gate script and sets `BREENIX_GATE_TMP` to a private absolute directory under the run directory. It does not recreate the QEMU command line in Swift and it does not compute a pass/fail verdict; the manifest records the gate script argv and exit code.

Host facts record QEMU peer counts separately for `qemu-system-aarch64` and `qemu-system-x86_64`, plus load average, host identity, QEMU version, thermal pressure when available, and git state. `[GATE_BOOT_FACTS]` serial ingestion is not wired up yet (lands in PR-7).

`show` renders a section per flag given (`--subsystems`, `--messages`, `--traces` may be combined), and defaults to `--subsystems` alone with no flag given: `--subsystems` walks the committed boot-stage catalog against the run's serial and prints each stage's reached/stopped state, `--messages` dumps each scanned serial line tagged with its marker family, and `--traces` reports which structured trace records (`GATE_BOOT_FACTS`, `BXCAP`, `FATAL_REGS`) are not wired up yet. `latest-fail` selects the most recent run whose verdict is a failure (`.fail`, `.attributed`, `.refused`, or a `.gateScript` whose exit code is not 0).
