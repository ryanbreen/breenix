# Breenix Runs

SwiftPM command-line tooling for preserving Breenix gate evidence and printing a host-facts trace for ARM gate runs.

## Build

```bash
swift build
swift test
```

`make app` is intentionally a stub in PR-1. SwiftUI app bundling lands in PR-6.

## CLI

PR-1 implements only:

```bash
breenix-runs run arm [strict|prod|testing] [--boots N] [--tag T] [--no-store]
breenix-runs facts <run-id|latest> [--json]
```

Examples:

```bash
swift run breenix-runs run arm strict --boots 20 --tag before-scheduler-change
swift run breenix-runs run arm --boots 1 --no-store
swift run breenix-runs facts latest
swift run breenix-runs facts latest --json
```

`run arm` drives the existing gate script and sets `BREENIX_GATE_TMP` to a private absolute directory under the run directory. It does not recreate the QEMU command line in Swift and it does not compute a pass/fail verdict; the manifest records the gate script argv and exit code.

Host facts record QEMU peer counts separately for `qemu-system-aarch64` and `qemu-system-x86_64`, plus load average, host identity, QEMU version, thermal pressure when available, and git state. `[GATE_BOOT_FACTS]` serial ingestion is not wired up in PR-1.
