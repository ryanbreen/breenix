# Breenix Runs

SwiftPM command-line tooling for preserving Breenix gate evidence and printing a host-facts trace for ARM (local QEMU) and x86 (beast Incus VM) gate runs.

## Build

```bash
swift build
swift test
```

`make app` now builds the debug `BreenixRunInspector` executable target and
creates an ad-hoc signed `Breenix Run Inspector.app` bundle in this directory.

## CLI and App

PR-1 through PR-8 implement:

```bash
breenix-runs run arm [strict|prod|testing] [--boots N] [--tag T] [--no-store]
breenix-runs run x86 [gate] [--boots N] [--sha SHA] [--mode kthread|full] [--host HOST] [--dry-run] [--tag T] [--no-store]
breenix-runs show <run-id|latest|latest-fail> [--subsystems] [--messages] [--traces]
breenix-runs facts <run-id|latest> [--json]
breenix-runs compare <run-id-a|latest|latest-fail> <run-id-b|latest|latest-fail>
breenix-runs tail [<run-id|latest|latest-fail>]
breenix-runs import <path>...
make app
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
swift run breenix-runs compare latest-fail latest
swift run breenix-runs tail latest
swift run breenix-runs import /tmp/breenix-gate-tree
make app
```

`run arm` drives the existing gate script and sets `BREENIX_GATE_TMP` to a private absolute directory under the run directory. It does not recreate the QEMU command line in Swift and it does not compute a pass/fail verdict; the manifest records the gate script argv and exit code.

`run x86` drives `docker/qemu/run-x86-gate.sh` on the beast Incus VM (`breenix-x86`, the only x86 build/boot host per `[[beast-x86-build-host]]`) rather than composing a QEMU command line locally: it makes a private `git clone --shared` of the container's canonical `/root/breenix` checkout at the sha under test (default: this repo's local HEAD; pass `--sha` to override), points `BREENIX_GATE_TMP` inside that clone, streams the gate's build+boot output back live, pulls the resulting evidence back over the same ssh channel, and removes the clone as its final teardown step on both a passing and a failing gate exit. `--sha` defaults to the local HEAD, so the branch under test must already be pushed to `origin` before running this — beast's canonical checkout fetches from GitHub, not from this Mac. A dirty local working tree prints a warning (beast tests the pushed commit, not uncommitted changes) rather than failing. `--dry-run` builds and prints that same plan (the four ssh/incus command lines) without running any of it: no ssh connection is opened and the run store's index is left unchanged. `--boots`/`--mode` map directly onto the gate script's own `[count] [mode]` arguments; `boot-tests` and `prod` (DESIGN.md 2.3's other two x86 CLI-surface entries) are not implemented by this PR.

Host facts record QEMU peer counts separately for `qemu-system-aarch64` and `qemu-system-x86_64`, plus load average, host identity, QEMU version, thermal pressure when available, and git state. This is the Inspector's own host-side sample (`HostFactsTrace`), distinct from the guest-annotated `[GATE_BOOT_FACTS]` records `--traces` reports below.

`show` renders a section per flag given (`--subsystems`, `--messages`, `--traces` may be combined), and defaults to `--subsystems` alone with no flag given: `--subsystems` walks the committed boot-stage catalog against the run's serial and prints each stage's reached/stopped state, `--messages` dumps each scanned serial line tagged with its marker family, and `--traces` decodes and prints three structured record families<!-- claim-lint:ok: #843 landed the first of these three families; DESIGN.md's §1.5 PR-7 status note and §4.4 PR-7 status note carry the file:line citations for all three decoders -->, each with an explicit "not present" line for a run that has zero of that family's records: `[GATE_BOOT_FACTS:boot=N:...]` host-facts records (read from the run's serial text plus its `gate-stdout.txt` capture — that second source is load-bearing today, per `DESIGN.md` §1.5's citation of exactly where the aarch64 gates write this line), `[FATAL_REGS]` postmortem records (both the labelled and unlabelled header shapes the aarch64 fault handlers emit, each with its full `x0`-`x30` register grid and dispatch trace), and `[BXCAP:...]` v1 kernel captures (no gate emits this yet, so this section reports "not present" on every run in this store at the time of writing; `truncated`/`refused`/interleaved-`seq` captures are exercised in `BXCAPTests.swift` against a fixture built by hand from the schema, labelled as synthesized there). `latest-fail` selects the most recent run whose verdict is a failure (`.fail`, `.attributed`, `.refused`, or a `.gateScript` whose exit code is not 0).

`compare` renders the same stored-run diff the app's Compare tab uses: subsystem stages whose reached/not-reached state differs, marker-family count deltas, host-facts start-sample deltas, and verdict text/state deltas. It accepts `latest` and `latest-fail` selectors on both sides.

`tail` follows the selected stored run's `gate-stdout.txt` capture when present, otherwise its first serial file, prints bytes as they are read, and returns after EOF is stable. Today's manifest-writing path records runs after the launcher exits, so this command does not attach to a live launcher process yet; it is a file-following primitive for stored evidence.

`import` records existing gate-tmp trees, preserved failure directories, and loose serial directories in the run store. Imported loose serials keep `.unknown` verdicts because the importer does not replay gate scoring.

`BreenixRunInspector` is a read-only macOS app for the same run store that the CLI
uses. PR-6 renders the run sidebar, subsystem state-machine rows, and scanned
serial messages; PR-7 adds a third "Traces" tab rendering the same host-facts,
`BXCAP`, and `FATAL_REGS` sections the CLI's `--traces` prints, each with its
own "not present" empty state. PR-8 adds a Compare tab after choosing a second
stored run. Launching runs remains in the CLI.
