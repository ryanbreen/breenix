# Breenix Runs

SwiftPM command-line tooling for preserving Breenix gate evidence and printing a host-facts trace for ARM (local QEMU) and x86 (beast Incus VM) gate runs.

## Build

```bash
swift build
swift test
```

`make app` is intentionally a stub in PR-1. SwiftUI app bundling lands in PR-6.

## CLI

PR-1 through PR-5 implement:

```bash
breenix-runs run arm [strict|prod|testing] [--boots N] [--tag T] [--no-store]
breenix-runs run x86 [gate] [--boots N] [--sha SHA] [--mode kthread|full] [--host HOST] [--dry-run] [--tag T] [--no-store]
breenix-runs show <run-id|latest|latest-fail> [--subsystems] [--messages] [--traces]
breenix-runs facts <run-id|latest> [--json]
breenix-runs import <path>...
```

Examples:

```bash
swift run breenix-runs run arm strict --boots 20 --tag before-scheduler-change
swift run breenix-runs run arm --boots 1 --no-store
swift run breenix-runs run x86 --boots 1
swift run breenix-runs run x86 --dry-run
swift run breenix-runs show latest
swift run breenix-runs show latest-fail --subsystems
swift run breenix-runs show <run-id> --messages --traces
swift run breenix-runs facts latest
swift run breenix-runs facts latest --json
swift run breenix-runs import /tmp/breenix-gate-tree
```

`run arm` drives the existing gate script and sets `BREENIX_GATE_TMP` to a private absolute directory under the run directory. It does not recreate the QEMU command line in Swift and it does not compute a pass/fail verdict; the manifest records the gate script argv and exit code.

`run x86` drives `docker/qemu/run-x86-gate.sh` on the beast Incus VM (`breenix-x86`, the only x86 build/boot host per `[[beast-x86-build-host]]`) rather than composing a QEMU command line locally: it makes a private `git clone --shared` of the container's canonical `/root/breenix` checkout at the sha under test (default: this repo's local HEAD; pass `--sha` to override), points `BREENIX_GATE_TMP` inside that clone, streams the gate's build+boot output back live, pulls the resulting evidence back over the same ssh channel, and removes the clone as its final teardown step on both a passing and a failing gate exit. `--sha` defaults to the local HEAD, so the branch under test must already be pushed to `origin` before running this — beast's canonical checkout fetches from GitHub, not from this Mac. A dirty local working tree prints a warning (beast tests the pushed commit, not uncommitted changes) rather than failing. `--dry-run` builds and prints that same plan (the four ssh/incus command lines) without running any of it: no ssh connection is opened and the run store's index is left unchanged. `--boots`/`--mode` map directly onto the gate script's own `[count] [mode]` arguments; `boot-tests` and `prod` (DESIGN.md 2.3's other two x86 CLI-surface entries) are not implemented by this PR.

Host facts record QEMU peer counts separately for `qemu-system-aarch64` and `qemu-system-x86_64`, plus load average, host identity, QEMU version, thermal pressure when available, and git state. `[GATE_BOOT_FACTS]` serial ingestion is not wired up yet (lands in PR-7).

`show` renders a section per flag given (`--subsystems`, `--messages`, `--traces` may be combined), and defaults to `--subsystems` alone with no flag given: `--subsystems` walks the committed boot-stage catalog against the run's serial and prints each stage's reached/stopped state, `--messages` dumps each scanned serial line tagged with its marker family, and `--traces` reports which structured trace records (`GATE_BOOT_FACTS`, `BXCAP`, `FATAL_REGS`) are not wired up yet. `latest-fail` selects the most recent run whose verdict is a failure (`.fail`, `.attributed`, `.refused`, or a `.gateScript` whose exit code is not 0).

`import` records existing gate-tmp trees, preserved failure directories, and loose serial directories in the run store. Imported loose serials keep `.unknown` verdicts because the importer does not replay gate scoring.
