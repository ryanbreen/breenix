# Breenix Run Inspector — the design

**Status (R186): this file is the approved design snapshot.** It was read
and written against `7a19f550` as described in §1 below, and it stays as
that fixed record instead of being re-verified each time a new commit
lands. The PR plan in §6 is what evolves the design from here: each landed
PR updates the parts of this document its own commit touches, and the
shipped Swift code — not this file — is the authority on current behavior
once a PR merges.

<!-- claim-lint:ok: the operator's own R185 request, reproduced verbatim
     exactly once (1 of 1) below -- not a claim this document makes -->
**Snapshot:** `7a19f550` (origin/main) — 2026-09-05 14:55 ET
**Operator ask (R185, verbatim):** *"a command line tool that will let me run arm or x86 and print me a trace of host facts. actually let's make it a swift app that will inspect traces of every run, preserving a list of every run and allowing us to see what subsystems initialized and what messages / traces we have for each."*

Toolchain verified on this Mac: Swift 6.3.3 (swiftlang-6.3.3.1.3), Xcode 26.6, macOS 26.6.2, arm64.

---

## 0. What this document commits to

<!-- claim-lint:ok: "every run" quotes the operator's own R185 ask reproduced
     above; "already exists on disk today" is the finding backed by
     docker/qemu/run-aarch64-prod-profile-boot-test.sh:246 and
     docker/qemu/run-aarch64-boot-test-strict.sh:510 in the paragraph directly
     below -->
The ask has two halves and they are **not** the same program. The CLI half ("run
arm or x86, print a trace of host facts") is a *launcher*. The app half ("inspect
traces of every run, preserving a list of every run") is a *reader*. The design
below makes the reader the primary artifact and the launcher a thin producer that
feeds it, because everything the app needs to read **already exists on disk today**
and is being thrown away.

That is the finding that shapes the whole plan: the gates already produce exactly
the evidence the operator wants to browse, and then either delete it
(`rm -rf "$OUTPUT_DIR"` at `docker/qemu/run-aarch64-prod-profile-boot-test.sh:246`)
or leave it in a `/tmp` directory the next run clobbers
(`docker/qemu/run-aarch64-boot-test-strict.sh:510`). Only *failed* boots are
preserved, and only as a bare serial
(`docker/qemu/run-aarch64-boot-test-strict.sh:484-505`). There is no list of runs,
no host facts per boot, and no way to ask "what did subsystem X do on the run I did
an hour ago". **The Run Inspector is mostly a matter of not throwing evidence away,
and then reading it well.**

---

## 1. Current-state read (verified at `7a19f550`)

This section was read in a clean worktree at `7a19f550`, and each claim in it
carries a `file:line`.

### 1.1 How runs are launched today

| Path | Launch site | Serial destination | Scoring |
|---|---|---|---|
| `run.sh` (interactive, ARM64) | `run.sh:1065-1080` — `-M virt,gic-version=3 -cpu max -smp 4 -m 512M`, `-serial mon:stdio` | operator's terminal | no scoring |
| `run.sh --x86` | `run.sh:1094-1115` — `-machine pc,accel=tcg -cpu qemu64 -smp 4`, `-serial mon:stdio` | operator's terminal | no scoring |
| `run.sh --parallels` | `run.sh:201` | `/tmp/breenix-parallels-serial.log` (fixed path, accumulates across boots) | no scoring |
| aarch64 strict gate | `docker/qemu/run-aarch64-boot-test-strict.sh:522-533` — `timeout 20`, `-cpu cortex-a72 -smp 4`, `-serial file:` | `$BREENIX_GATE_TMP/breenix_aarch64_strict_<i>/serial.txt` | `score_serial()`, ~40 pinned markers |
| aarch64 prod-profile gate | `docker/qemu/run-aarch64-prod-profile-boot-test.sh:300-313` — `timeout 120`, `-cpu max -smp 4` | `$BREENIX_GATE_TMP/breenix_aarch64_prod_profile/serial.txt` | marker counts, `:186-208` |
| aarch64 testing-profile gate | `docker/qemu/run-aarch64-testing-profile-boot-test.sh:222-236` — `-cpu max -smp 4` | `$OUTPUT_ROOT/<i>/serial.txt` + `qemu-stdout.log` | `classify_serial()`, `:79-154` |
| x86 frame-custody gate | `docker/qemu/run-x86-boot-tests.sh:445-459` — `-smp 1`, **two** `-serial file:` | `serial_user.txt` (COM1) + `serial_kernel.txt` (COM2) | `scripts/x86-gate-verdict.sh` |
| x86 merge gate | `docker/qemu/run-x86-gate.sh:162-164` | `$BREENIX_GATE_TMP/breenix_gate_<i>/serial_{user,kernel}.log` | verdict script in `full` mode |
| x86 prod-profile gate | `docker/qemu/run-x86-prod-profile-boot-test.sh:1042` | `$BREENIX_GATE_TMP/breenix_x86_prod_profile` | `:1292` |

**Two facts from this table drive the design.**

1. **x86 emits two serial streams, aarch64 one.** On x86, `log::*` output goes to
   COM2 and userspace to COM1 (`docker/qemu/run-x86-boot-tests.sh:457-458`). The
   data model must hold *a set of* serial streams per run, not one file. The x86
   gate's own scorers already read them as a set (`"$OUTPUT_DIR"/serial_*.txt`).
2. **`BREENIX_GATE_TMP` is the universal escape hatch, and it is already
   everywhere** — `run-aarch64-boot-test-strict.sh:31`,
   `run-aarch64-prod-profile-boot-test.sh:21`,
   `run-aarch64-testing-profile-boot-test.sh:43`, `run-x86-boot-tests.sh:79`,
   `run-x86-gate.sh:63`, each with an absolute-path guard (finding F6 on #797). This
   is the launcher's entire integration surface: **point `BREENIX_GATE_TMP` at a
   per-run directory the Inspector owns, and the gate writes its evidence into the
   store instead of into a shared `/tmp` path.** No gate needs modifying to be
   captured. This also discharges #825 by construction for Inspector-launched runs.

### 1.2 The gates already have offline scorers — do not reimplement scoring

The three aarch64 gates can each score a serial **that already exists**, without booting:

* `BREENIX_STRICT_SCORE_ONLY` — `run-aarch64-boot-test-strict.sh:157`, entry point
  at `:469-482`. Prints `SCORE: PASS - <path>` or `SCORE: FAIL - <reason> (<path>)`.
* `BREENIX_PROD_SCORE_ONLY` — `run-aarch64-prod-profile-boot-test.sh:131-139`.
* `--classify <serial>...` — `run-aarch64-testing-profile-boot-test.sh:167-184`.

The strict gate's comment at `:461-468` says why they exist: so the scoring rules
can be exercised against a preserved serial without booting.

**Design consequence, and it is the most important one in this document:** the
design requires that the Inspector **not re-implement a gate verdict in Swift**. A
Swift reimplementation would be a second scorer that can silently disagree with the
gate that decides merges — precisely the "gate scripts build ONLY
`aarch64-breenix-kernel.json`" class of drift the tree has been bitten by three
times (#549, #551, #527-r1). The Inspector *shells out to the gate's own scorer* and
records its stdout and exit status verbatim as the verdict. Swift parses serials for
**display and navigation**; the shell script remains the sole authority on
pass/fail.

### 1.3 What a serial actually carries

Grounded against a real committed serial,
`docs/planning/green-program/aarch64-testing/serials/slice3d/05-runtime-anti-vacuity-strict-serial.txt`:

* **Markers are not line-anchored.** That file contains the literal
  `T2T3[BOOT_TESTS:` — the scheduler's single-character trace stream is interleaved
  on the same UART. The x86 side documents the same hazard with a worked example at
  `docker/qemu/run-x86-prod-profile-boot-test.sh:595`
  (`<S>[SW]<K>[SW]<T><U><R>[TTY_ORACLE:FAIL:...`). The strict gate says so
  explicitly at `:44` and `run-x86-boot-tests.sh:96-98`: *"any marker line can carry
  a prefix… the markers are self-delimiting… do not re-anchor these."*
  **The design requires each scanner in the Inspector to match a bracketed token
  anywhere in a line.**
* **Markers repeat.** `[BOOT_TESTS:PASS]` appears at lines 602 *and* 610 of that
  file. The gates use `grep -c` semantics (`run-aarch64-prod-profile-boot-test.sh:147`).
  The Inspector is required to store each occurrence with its line number, and
  expose counts.
* Serial files are binary-ish: each gate greps them with `-a`
  (`run-aarch64-testing-profile-boot-test.sh:73-74`). Swift must read them as
  bytes and decode lossily, and must not use `String(contentsOf:encoding:.utf8)`.

### 1.4 The subsystem-init model already exists in the tree

`xtask/src/boot_stages.rs` defines `BootStage { name, marker, failure_meaning,
check_hint }` (`:11-16`) and **280 stage entries** across
`x86_64_kernel_stages()` (`:91`), `arm64_kernel_stages()` (`:440`),
`shared_userspace_stages()` (`:575`) and `x86_64_extra_stages()` (`:1603`).
Matching semantics, from `xtask/src/main.rs:668-673`: plain substring
`contents.contains(marker)`, with `|` in a marker meaning *alternatives*.

This is the Subsystems pane's content, already written, already arch-aware, already
carrying the "what it means when this fails" text the operator would want on hover.
**The Inspector must consume this catalog rather than copy it into Swift.** A
duplicated 280-entry list is a second source of truth that goes stale. The
precedent for doing this right is in the tree: `scripts/trace_memory_dump.py:16-22`
reads its layout constants and event table *out of the kernel sources* rather than
duplicating them, and says why — a stale copy silently mis-decoded most events.

The raw `[boot]` line family is the other half. `kernel/src/main_aarch64.rs`
carries **87** `[boot]` lines, in a consistent shape:
`[boot] Initializing X...` (`:539`, `:563`, `:573`, `:581`, `:628`, `:636`, `:642`,
`:714`, `:836`, `:857`, `:862`, `:948`) paired with a terminator —
`[boot] X ready` (`:547`), `[boot] X initialized` (`:583`, `:600`, `:859`, `:864`,
`:870`, `:877`, `:950`), `[boot] X mounted` (`:647`, `:670`), or a failure arm
`[boot] X failed: {e}` / `[boot] X init failed: {e}` (`:652`, `:753`, `:758`,
`:768`, `:786`, `:795`, `:826`) / `[boot] No display device found` (`:800`).

x86 does **not** use this shape: `kernel/src/main.rs` uses `log::info!`
(`:129`, `:149`, `:376`, `:458`, `:463`, `:592`), which the logger renders as
`<ts> - [<LEVEL>] <target>: <msg>` (`kernel/src/logger.rs:1030-1046`, with a
`{:>5}` right-aligned level). **The two arches need different line grammars**, which
is exactly why the stage catalog is arch-keyed already.

### 1.5 The two formats landing today from sibling lanes

<!-- claim-lint:ok: `git grep -e GATE_BOOT_FACTS -e BXCAP -e qemu-host-lock
     7a19f550` reproduces the result -- 0 of 3 search terms had a hit outside
     `.git` at the snapshot commit named at the top of this document, before
     this tool's own Sources/ and README added the same three strings -->
Neither exists at `7a19f550`. I grepped the whole worktree for `GATE_BOOT_FACTS`,
`BXCAP` and `qemu-host-lock` and got **zero hits outside `.git`**. Both are
in-flight:

* **`[GATE_BOOT_FACTS:boot=N:host_ms=…:qemu_at_start=…:load_at_start=…:qemu_at_end=…:load_at_end=…:qemu_cpu_s=…:guest_uptime_ms=…:ended_by=…]`** (#827).
  The failure-capture plan's PR-1 also writes these into a `.facts` sidecar beside
  the preserved serial (`FAILURE-TRACE-CAPTURE-PLAN-2026-09-05.md` §PR-1). Note the
  two carriers can disagree in field set; the Inspector reads both.
* **`[BXCAP …]` v1** — schema at `FAILURE-TRACE-CAPTURE-PLAN-2026-09-05.md` §4:
  `BEGIN/EDGE/CPU/THR/DISP/EV/CNT/RING/NOTE/END`, `TOKEN key=value` throughout,
  `v=` on `BEGIN` and `END`, `seq` globally monotonic, `BEGIN` without `END` **is
  the definition of a truncated capture**, and a decoder **refuses an unknown major
  version rather than mis-decoding**.

**Design consequence:** the Inspector is written against formats that do not exist
yet, so it must degrade to *"this run has no host facts"* / *"this run has no
capture"* as a first-class displayed state — not an empty pane, and not a
fabricated number it did not measure. And the `GATE_BOOT_FACTS` parser is
**generic over its key set**:
it splits `key=value` pairs after `boot=` rather than pinning the eight fields
above, so a field added or renamed while #827 lands shows up as a new row instead of
breaking the parse. The BXCAP decoder does the opposite — it *refuses* an unknown
`v=`, because that is what the schema mandates.

### 1.6 Host rules that constrain the launcher

* **x86 builds and boots run on beast**, not this Mac
  (`[[beast-x86-build-host]]`; `docker/qemu/run-x86-gate.sh:5-6` calls it "the gate
  that guards merges on the beast x86 VM"). `run-x86-gate.sh` is already
  parameterised for exactly this: `BREENIX_REPO_DIR`, `BREENIX_QEMU_ACCEL`,
  `BREENIX_QEMU_CPU`, `BREENIX_GATE_TIMEOUT`, `BREENIX_GATE_TMP` (`:26-40`). Its
  header states what cannot live in the repo (`:42-47`): the fetch/checkout of the
  branch under test, because a script that `git reset --hard`s the checkout it is
  being read from is a self-modification hazard. **That is the Inspector's job on
  the x86 path**, and it is the reason the x86 launcher is a real component rather
  than a one-line `ssh`.
* **aarch64 runs locally, one boot at a time.** Max ~4 concurrent QEMUs on this Mac
  (`[[feedback-qemu-gate-concurrency]]`); a host-wide lock
  (`docker/qemu/lib/qemu-host-lock.sh`) lands today. The Inspector's local launcher
  **acquires that lock when it exists and refuses to launch when it does not exist
  and another `qemu-system-aarch64` is already running** — it must not be the
  process that reintroduces the #826 host-contention confound.
* **Evidence must not share fixed `/tmp` paths (#825).** Satisfied by
  construction: each Inspector-launched run gets its own `BREENIX_GATE_TMP`.

---

## 2. Product

### 2.1 Location: `tools/breenix-runs/` — justified

The repo root today holds sixteen `breenix-*` directories; **16 of 16 are Claude
skill directories** (each is a `SKILL.md`, e.g. `breenix-boot-analysis/SKILL.md`;
`breenix-gdb-chat/` is `SKILL.md` + `scripts/`) — `ls -d breenix-*/` plus a
`SKILL.md` check per directory reproduces the count. Putting a Swift package at
`breenix-run-inspector/` would land it in the middle of the skill namespace and
read as a seventeenth skill.

`tools/` does not exist yet, which is a feature: it is an unambiguous new home for
developer tooling that is neither a skill (`breenix-*/`), a shell script
(`scripts/`), a gate (`docker/qemu/`), nor a cargo crate.

The decisive check: **`Cargo.toml`'s `[workspace] members` is an explicit list —
`kernel`, `parallels-loader`, `xtask`** (`Cargo.toml:54-59`). It is not a glob, so a
new top-level directory containing non-Rust code cannot be swept into the cargo
build, and `cargo build` at the root stays exactly as fast and as clean as it is
today. A Swift package under `tools/` does not touch the cargo build, the gate
scripts, or any other workflow already in this repo.

<!-- claim-lint:ok: checked directly against this repo's `.gitignore`, which
     already carries both additions at its end and does not collide with
     /target/, build/, or .worktrees/ above them -->
`.gitignore` needs two additions (`tools/breenix-runs/.build/`, `tools/breenix-runs/*.app/`);
the current ignore file's existing entries (`/target/`, `build/`, `.worktrees/`)
do not collide with either addition.

### 2.2 Three targets

```
tools/breenix-runs/
├── Package.swift
├── Makefile                       # `make app` — the bundling step (§2.4)
├── README.md
├── Sources/
│   ├── BreenixRuns/               # library: store, ingestion, launchers
│   │   ├── Store/                 RunStore, RunManifest, RunIndex, Importer
│   │   ├── Parsing/               MarkerScanner, families, SerialIndex,
│   │   │                          BXCAPDecoder, FatalRegsDecoder, BootFacts
│   │   ├── Subsystems/            StageCatalog (loaded from xtask export), StateMachine
│   │   └── Launch/                LocalGateLauncher, BeastLauncher, HostFacts
│   ├── breenix-runs/              # executable: the CLI (the operator's first ask)
│   └── BreenixRunInspector/       # executable: the SwiftUI app
└── Tests/
    ├── BreenixRunsTests/
    └── Fixtures/                  # real serials, committed (§5, PR-2)
```

Why a library plus two executables rather than one app with a CLI mode: the CLI must
run headless over SSH and inside a workflow agent, where an `NSApplication` is a
liability. Splitting them means the CLI target links no AppKit/SwiftUI at all, and
`swift test` exercises the library — the part that carries the application logic —
with no GUI
in the loop.

### 2.3 CLI surface

```
breenix-runs run arm [strict|prod|testing] [--boots N] [--tag T] [--no-store]
breenix-runs run x86 [gate|boot-tests|prod] [--boots N] [--host beast]
breenix-runs list   [--arch arm|x86] [--verdict pass|fail] [--since 2d] [--limit N]
breenix-runs show   <run-id|latest|latest-fail>   [--subsystems] [--messages] [--traces]
breenix-runs facts  <run-id|latest>               [--json]
breenix-runs tail   [<run-id>]                    # follow a live run's serial
breenix-runs import <path>...                     # a gate tmp tree, or a serials dir
breenix-runs score  <run-id>                      # re-run the gate's own scorer
```

`run arm` with no profile defaults to `strict` — it is the kernel-merge gate
(`run-aarch64-boot-test-strict.sh:5-8`) and the one whose evidence is most worth
keeping. `facts <run>` is the operator's literal ask ("print me a trace of host
facts") and prints, in order: the run header (arch, profile, kernel BUILD_ID, git
sha, image sha256), the Inspector's own host sample (§4.3), and then each
`GATE_BOOT_FACTS` record found in the serial — or an explicit
`no [GATE_BOOT_FACTS] records in this serial (#827 not landed on this run's gate)`
line when no such record exists.

### 2.4 Build and launch: SwiftPM + `make app`, no Xcode project — justified, and verified

**Decision: SwiftPM executable target + a `make app` bundling step. No `.xcodeproj`,
no xcodegen.**

I verified both halves on this machine rather than assuming them:

1. A SwiftPM `executableTarget` with `platforms: [.macOS(.v14)]` importing SwiftUI
   and declaring `@main struct App: App` **builds clean** — `swift build` →
   `Build complete! (5.05s)`, and SwiftPM itself emits an entitlement plist and runs
   a codesign step.
2. Copying that binary into a hand-written `.app` skeleton
   (`Contents/MacOS/<exe>` + `Contents/Info.plist` with `CFBundleExecutable`,
   `CFBundleIdentifier`, `CFBundlePackageType=APPL`, `NSHighResolutionCapable`),
   ad-hoc signing it (`codesign --force --sign -`), and running `/usr/bin/open` on
   it **launches a real GUI app** — confirmed by `pgrep -l Probe` returning a live
   process after `open`.

So `make app` is ~15 lines of `mkdir`/`cp`/heredoc/`codesign`, and it is the whole
build system. Why this over the alternatives:

* **vs. a committed `.xcodeproj`:** an Xcode project is a large generated XML blob
  prone to merge conflicts, cannot be reviewed meaningfully, and pins Xcode versions. This
  repo has no GUI-app precedent to inherit and no CI to run Xcode on (there are no
  GitHub Actions here).
* **vs. xcodegen:** adds a Homebrew dependency and a `project.yml` to produce a
  project we would then have to gitignore — most of the tooling cost of an Xcode
  project without its benefit, when `swift build` already does the compile.
* **The cost, disclosed:** no Xcode previews, no Interface Builder, no
  Instruments-by-scheme. For a document-shaped inspector app this is a fair trade;
  if SwiftUI previews later become important, adding a generated project is a
  reversible decision that changes no source.

Ad-hoc signing means Gatekeeper will treat the bundle as unsigned; since the
operator builds it locally from source and launches it from the build directory,
that is correct and requires no notarization story.

### 2.5 App screens

**Sidebar** — each run, newest first. One row per run:

```
┌──────────────────────────────────────────────┐
│  ● arm  strict    20/20      7a19f550  14:52 │   ● green
│  ● arm  prod       PASS      7a19f550  14:31 │
│  ● x86  gate      FAIL 2/4   681c1d58  13:07 │   ● red
│  ◌ arm  strict   running…    (local)   13:02 │   ◌ in flight
│  ● arm  testing  PASS+#728   f96ea36c  11:44 │   ● amber = attributed
└──────────────────────────────────────────────┘
```

Badges are arch / profile / verdict / short-sha. The amber state is not decoration:
the testing-profile gate has a real third verdict,
`PASS-WITH-ATTRIBUTED-LOCKUP` (`run-aarch64-testing-profile-boot-test.sh:246`), and
collapsing it to green would hide exactly what that gate exists to report.

**Detail — four panes.**

*Subsystems.* The stage catalog for this run's arch, in boot order, each row
`reached | failed | not reached`, with the serial line number where its marker first
appeared and the delta from the previous stage. `failed` rows carry
`BootStage.failure_meaning` and `check_hint` verbatim
(`xtask/src/boot_stages.rs:14-15`). The first `not reached` stage after a run of
`reached` ones is the headline: *"boot stopped here"*.

```
Subsystems — arm64, strict            reached 41 / 47   stopped at #42
─────────────────────────────────────────────────────────────────────
✓ ARM64 kernel starting            L12    +0ms
✓ Memory management ready          L28   +14ms
✓ Timer calibrated                 L31    +2ms
✓ GIC initialized                  L39    +7ms
…
✓ Scheduler initialized           L188   +31ms
✗ Init pre-loaded from ext2        —      —     ← boot stopped here
    means: ext2 root fs not mounted or /sbin/init missing
    check: kernel/src/main_aarch64.rs ext2 pre-load path
○ Timer interrupt initialized      —      —
```

*Messages.* Each serial line, with a family chip, filterable by family
(boot / tests / oracles / heartbeat / faults / trace-noise / other), free-text
search, and a "hide heartbeats" toggle — the gates themselves filter heartbeats out
when finding the last real line (`run-aarch64-service-sequence-gate.sh:870`), which
is a good default. Two streams are shown side-by-side on x86 (COM1/COM2), merged by
file order, each labelled — not silently concatenated.

*Traces.* Structured records only, three sections, each absent-by-default with an
explicit "not present" state:

```
Host facts (#827)          8 boots ·  from serial
  boot host_ms guest_ms ratio qemu@start load@start qemu_cpu_s ended_by
     1   4820     4771  0.99          1       2.14       3.9   scored_pass
     2   4901     2210  0.45          5       9.87       1.2   poll_exhausted   ⚠

Kernel capture (BXCAP v1)  seq 1 · edge PANIC · verdict=partial · truncated=1
  ▸ EDGE   ▸ CPU ×4   ▸ THR ×11   ▸ DISP ×16   ▸ EV ×512   ▸ RING ×4
  per-CPU timeline ────────────────────────────────────────────────
   cpu0 ▏▍▍▎▏ ▏▎▍ ▊▊▊          ← EV events, per-CPU slot order
   cpu1 ▏▏  ▎▏▍▎▏▏
   (merged order is approximate — per-CPU slot order is authoritative)

Fatal registers            1 record
  label=INSTRUCTION_ABORT cpu=3 spsr=0x60000005 esr=0x8600000e
  far=0xffff000041139200 elr=0x0 sp=0xffff0000431fff70
  x0…x30 grid + DISPATCH_TRACE cpu=3
```

The "merged order is approximate" caption is not a nicety — the BXCAP schema
explicitly declines to promise a globally total event order
(`FAILURE-TRACE-CAPTURE-PLAN-2026-09-05.md` §4, "Deliberately excluded from v1"),
and `scripts/trace_memory_dump.py` documents that nested interrupt writers can
commit out of timestamp order. A timeline that implied otherwise would be the app
lying.

*Compare.* Two runs side by side: subsystem-state delta (stages one reached and the
other did not), marker-set delta (families/counts present in one only), host-facts
delta, and verdict delta. This is the "was it my change or the host?" view, and it
is why host facts and subsystem states live in the same store.

---

## 3. Data model and storage

### 3.1 The run record

```swift
struct RunManifest: Codable {          // runs/<id>/manifest.json — schemaVersion 1
    let id: String                     // <utc>-<arch>-<profile>-<rand4>, sorts newest-last
    let startedAt: Date                // UTC; displayed ET
    let endedAt: Date?
    let arch: Arch                     // .aarch64 | .x86_64
    let profile: String                // strict | prod | testing | gate | boot-tests | interactive
    let launcher: Launcher             // .localQEMU | .beastSSH | .gateScript | .imported
    let kernel: KernelIdentity         // buildID?, gitSHA?, gitDirty?, imageSHA256?
    let host: HostFacts?               // the Inspector's own sample (§4.3)
    let verdict: Verdict               // .pass | .fail(String) | .attributed(String)
                                       //   | .running | .unknown
    let verdictSource: VerdictSource   // .gateScript(cmd, exitCode) | .imported | .none
    let serials: [SerialRef]           // [{ name, path, bytes, stream: com1|com2|single }]
    let captures: [CaptureRef]         // .facts sidecars, qemu.log, gate stdout
    let command: [String]              // exact argv, for reproduction
    let env: [String: String]          // the gate-relevant subset (BREENIX_GATE_TMP, …)
    let tags: [String]
    let notes: String?
}
```

Notes on three fields that are easy to get wrong:

* **`verdict` is not computed by Swift for a gate run.** It is
  `.gateScript(cmd, exitCode)` carrying the script's own stdout. `.unknown` is a real
  value and is displayed as such. For imported evidence with no recorded exit status,
  the Inspector offers `breenix-runs score <run>` to obtain one from the gate's
  offline scorer (§1.2) — and records *that* invocation as the verdict source.
* **`kernel.buildID`** comes from the boot banner
  (`kernel/src/main_aarch64.rs:488`, format from `kernel/build.rs:28-33`: a
  14-hex-digit timestamp). It is **aarch64-only** — `BREENIX_BUILD_ID` has exactly
  one consumer in the tree and it is that line. On x86 the field is `nil`, and the
  UI says "no BUILD_ID banner on this arch" rather than showing a blank.
* **`serials` is a list.** x86 runs have two (§1.1).

### 3.2 Storage: files of record + a rebuildable index. Not SQLite.

```
~/Library/Application Support/BreenixRuns/
├── index.json                 ← DERIVED cache; delete it and it rebuilds
├── schema-version
└── runs/
    └── 20260905T185233Z-aarch64-strict-a3f1/
        ├── manifest.json
        ├── serial.txt                  (or serial_user.txt + serial_kernel.txt)
        ├── boot-facts.jsonl            extracted GATE_BOOT_FACTS, one per boot
        ├── gate-stdout.txt             the scorer's own words, verbatim
        ├── qemu.log
        └── captures/…                  .facts sidecars, BXCAP extracts
```

**Why not SQLite:**

<!-- claim-lint:ok: each of the six gate scripts in the §1.1 table above
     (lines 44-49 there) preserves a serial file at the "Serial destination"
     path cited in that row; run-aarch64-boot-test-strict.sh:496-503 below is
     this item's own worked example -->
1. **The evidence is already files, and the tree's culture is that the file is the
   record.** Every gate preserves a serial *file* (`run-aarch64-boot-test-strict.sh:496-503`,
   which preserves an *empty* file when QEMU never opened one, so that "zero serial
   bytes" — the #569 silent-hang signature — is on the record). A database would
   become a second source of truth that can disagree with the serial next to it, and
   there is no way for a reviewer to `grep` it.
2. **Rebuildability is the whole safety property.** `index.json` is a cache derived
   by scanning `runs/*/manifest.json`. Delete it, corrupt it, hand-edit it — it
   regenerates. A corrupt SQLite file is a support incident.
3. **Two writers, no locking story needed.** The CLI and the app both write. With
   files, each run directory is written once by its owner (`manifest.json.tmp` →
   atomic `rename(2)`), and any process may rebuild the index. SQLite would need WAL
   plus a considered concurrency design to get the same property.
4. **The blobs never belong in rows.** The largest committed serial in the tree is
   1.7 MB (`docs/planning/green-program/fs/serials/x86-armed-serial-20260828.txt`).
   Serials get memory-mapped and line-indexed lazily; they are never loaded into a
   record.
5. **Scale does not demand it.** The realistic ceiling is thousands of runs. An
   index row is ~300 bytes, so 10 000 runs is a ~3 MB JSON read at launch.

**When this decision should be revisited, stated up front so it is a decision and
not an assumption:** if cross-run full-text search over serial *bodies* becomes a
requirement, add SQLite **FTS5 as a second derived, rebuildable index** — still not
the record. That is an additive change to one component.

### 3.3 Importing existing evidence

History must not be lost, and there is a lot of it: **2 725 committed `.txt` serials
under `docs/planning/**/serials/`**, plus whatever `BREENIX_GATE_TMP` trees are on
disk.

`breenix-runs import <path>` handles three shapes:

| Shape | Detection | Inferred |
|---|---|---|
| A gate tmp tree (`breenix_aarch64_strict_<i>/`, `breenix_x86_boot_tests_<i>/`, `breenix_gate_<i>/`) | directory name pattern + a `serial*.txt` inside | arch and profile from the directory name; one run per iteration directory |
| A preserved-failure dir (`breenix_aarch64_strict_failures/`, `breenix_prod_profile_failures/<ts>/`, `breenix_testing_profile_failures/`) | `<utc>-boot<N>.txt` filename (`run-aarch64-boot-test-strict.sh:493`) | `startedAt` from the filename stamp; `verdict = .fail("imported")` |
| A loose serials directory (`docs/planning/**/serials/`) | any `*.txt` | arch from banner (`Breenix ARM64 Kernel Starting`) or from `serial_kernel` naming; profile from marker presence (`boot_tests`-only markers ⇒ strict profile, per the census at `run-aarch64-boot-test-strict.sh:203`) |

Import is **idempotent**: a run's id is derived from
`sha256(serial bytes) || sourcePath`, so re-importing the same tree updates rather
than duplicates. Imported runs are tagged `imported` and their `verdict` is
`.unknown` unless the source directory recorded one — importing must not invent a
verdict, which is the same rule as §3.1.

---

## 4. Ingestion

### 4.1 Scanner design

One pass over the serial bytes produces a `SerialIndex`: for each line, its byte
range, and any number of `MarkerHit { family, range, fields, lineNumber }` (0..*).

Three rules, each forced by evidence in §1.3:

1. **Do not anchor to line start.** Each family's regex is applied with a
   *search*, not a match. Verified need: `T2T3[BOOT_TESTS:`.
2. **Decode lossily, work in bytes.** `String(decoding:as:UTF8.self)` per line
   after splitting on `\n`, mirroring the gates' `grep -a`.
3. **Record each occurrence.** Counts are a derived query, matching `grep -c`.

### 4.2 Marker families — the table

Regexes below are Swift/ICU literals. Where a gate script already owns a pattern,
the Inspector uses **the gate's pattern verbatim** and cites it, so the two cannot
drift.

| Family | Regex (search, unanchored) | Fields | Source |
|---|---|---|---|
| `bootStage.aarch64` | `\[boot\] (.+)` | text | `kernel/src/main_aarch64.rs` (87 sites, e.g. `:539`, `:547`, `:583`, `:950`) |
| `bootBanner.aarch64` | `Breenix ARM64 Kernel Starting` / `BUILD_ID: ([0-9a-f]{14})` | buildID | `kernel/src/main_aarch64.rs:486-488`; `kernel/build.rs:28-33` |
| `kernelLog.x86` | `(?:(\d+) - )?\[([A-Z ]{5})\] ([^:]+): (.*)` | ts, level, target, msg | `kernel/src/logger.rs:1030-1046` |
| `test.case` | `\[TEST:([^:\]]+):([^:\]]+):(START\|PASS\|TIMEOUT\|PANIC\|FAIL:[^\]]*\|DEFERRED:#\d+)\]` | suite, name, state, detail | `kernel/src/test_framework/executor.rs:782,790,794,799,804,766` |
| `test.complete` | `\[TESTS_COMPLETE:(\d+)/(\d+)(?::VACUOUS)?(?::FAILED:(\d+))?\]` | completed, total, failed | `executor.rs:270,273,276,312` |
| `test.bootTests` | `\[BOOT_TESTS:(START\|PASS\|SKIP\|TOTAL:\d+\|SERIAL_BOOT:\d+\|EARLY_BOOT:\d+\|STAGED:[^\]]*\|FAIL:[^\]]*)\]` | state | `executor.rs:271,274,278,296,311,328-333` |
| `test.ktap` | `^(?:not )?ok (\d+) (.+?)(?: # (SKIP\|TIMEOUT))?$` / `KTAP version 1` / `1\.\.(\d+)` | num, name, disposition | `kernel/src/test_framework/ktap.rs:22-54` |
| `test.btrt` | `\[btrt\] Boot Test Result Table at phys (0x[0-9a-f]+) \((\d+) bytes\)` / `===BTRT_READY===` | phys, size | `kernel/src/test_framework/btrt.rs:205-209,329` |
| `heartbeat` | `\[heartbeat\] tid=(\d+) uptime_ms=(\d+) kbd_nonzero=(\d+)` | tid, uptimeMs, kbd | `userspace/programs/src/heartbeat.rs:168` |
| `execSmoke` | `\[EXEC_SMOKE:([A-Z_]+)(?: ([^\]]*))?\]` | state, detail | `userspace/programs/src/exec_smoke.rs:11,20`; `exec_smoke_target.rs:13,18,34`; `init.rs:285,288` |
| `oracle.generic` | `\[([A-Z][A-Z0-9_]*(?:_ORACLE\|_CENSUS)):([^\]]*)\]` → split payload on `:` into `k=v` | name, k/v map | shape shared across the oracle/census rows in this table |
| `oracle.futexHandoff` | gate literal | 13 fields | `run-aarch64-boot-test-strict.sh:47` / `run-x86-boot-tests.sh:129` |
| `oracle.fcntlPM` | gate literal | 9 fields | `run-aarch64-boot-test-strict.sh:72` |
| `oracle.irqHold` | gate literal | 11 fields | `run-aarch64-boot-test-strict.sh:105` |
| `oracle.pollTCP` | `\[POLL_TCP_ORACLE:` + `:FAIL` arm; `\[POLL_TCP_TIMEOUT\]`; `\[POLL_TCP_READY_LOST\]` | state | `run-aarch64-boot-test-strict.sh:327-349` |
| `oracle.timerScale` | `\[TIMER_SCALE_ORACLE:x86:ms_per_tick=5:…:PASS\]` | 6 fields | `run-x86-boot-tests.sh:364` |
| `census.ttbr0ASID` | `\[TTBR0_ASID_CENSUS:untagged=(\d+):tagged=(\d+):kernel=(\d+):cleared=(\d+)\]` | 4 counters | `run-aarch64-boot-test-strict.sh:134` |
| `census.pinnedHome` | `\[PINNED_HOME_CPU_UNAVAILABLE:count=(\d+):publish_discarded=(\d+):hold_pen_migrated=(\d+):delivered=(\d+)\]` and the `:first:` variant | 4 counters | `run-aarch64-boot-test-strict.sh:107-109` |
| `census.strand` | `\[SCHED_STRAND_ORACLE:` / `\[STRAND_INJECT_ORACLE:` / `\[CENSUS_WIDEN_ORACLE:` | k/v map | `run-aarch64-boot-test-strict.sh:49,51,122` |
| `fault.fatalRegs` | `\[FATAL_REGS\](?: label=(\S+))? cpu=(\d+) spsr=(0x[0-9a-f]+) esr=(0x[0-9a-f]+) far=(0x[0-9a-f]+) elr=(0x[0-9a-f]+) sp=(0x[0-9a-f]+)` — **multi-line record**, see §4.4 | label?, cpu, 5 regs, x0…x30, dispatch trace | `kernel/src/arch_impl/aarch64/exception.rs:224-251` (labelled) and `:295` (unlabelled) |
| `fault.el1First` | `\[UNHANDLED_EC\] cpu=(\d+) EC=(0x[0-9a-f]+) ELR=(0x[0-9a-f]+)` / `\[EL1_FIRST_FAULT\] instruction_word=(\S+) …` | ec, elr, sysregs | `exception.rs:266-290` |
| `fault.abort` | `\[(DATA\|INSTRUCTION)_ABORT\].*from_el0=([01])` | kind, fromEL0 | `run-aarch64-testing-profile-boot-test.sh:92-93` |
| `fault.panic` | `panicked at kernel/src/` (kernel) vs `thread '.*' panicked at ` (userspace) | scope | `run-aarch64-testing-profile-boot-test.sh:61,91` |
| `fault.softLockup` | `!!! SOFT LOCKUP DETECTED !!!` | — | `run-aarch64-testing-profile-boot-test.sh:58` |
| `fault.ext2Stall` | `EXT2_LOCK_SPIN_STALL` | — | `run-aarch64-testing-profile-boot-test.sh:59` |
| `lockOrder` | `\[(EXEC\|CREATION)_LOCK_ORDER:VIOLATION` | which | `run-aarch64-boot-test-strict.sh:252,258` |
| `device.pciCensus` | `PCI: Enumeration complete\. Found (\d+) devices \((\d+) VirtIO block, (\d+) network\)` | 3 counts | `docker/qemu/run-x86-gate.sh:202-203` |
| `hostFacts` | `\[GATE_BOOT_FACTS:boot=(\d+):(.*)\]` → payload split into `k=v` **generically** | boot + open k/v map | #827, landing today (absent at `7a19f550`) |
| `capture.bxcap` | `\[BXCAP:(BEGIN\|EDGE\|CPU\|THR\|DISP\|EV\|CNT\|RING\|NOTE\|END) ([^\]]*)\]` → `k=v` | record type + fields | `FAILURE-TRACE-CAPTURE-PLAN-2026-09-05.md` §4 (absent at `7a19f550`) |
| `traceNoise` | run of single chars outside any bracketed token | — | scheduler trace stream, §1.3 |

### 4.3 The subsystem-init state machine

**Catalog source.** A new `cargo run -p xtask -- dump-boot-stages --arch <a> --json`
emits the `BootStage` list as JSON. The Inspector loads that file. It is regenerated
by `make catalog` and committed under `tools/breenix-runs/Resources/`, so the app
works without a cargo toolchain, and a Rust test asserts the committed JSON matches
the live catalog — **census-anchored on the entry count and name set, not a
literal list of stage names** (`[[#549/#551 rule]]`; the tree has been bitten three
times by literal lists in ratchets).

**States, and what each one means:**

| State | Rule |
|---|---|
| `reached` | the stage's marker (or, with `\|`, any alternative) occurs in the concatenated streams — same predicate as `xtask/src/main.rs:668-673` |
| `failed` | the stage is `notReached` **and** a failure arm for the same subsystem is present — `[boot] <X> failed:` / `[boot] <X> init failed:` / `[boot] No <X> found` (`kernel/src/main_aarch64.rs:652,753,786,795,800,826`) |
| `notReached` | no marker, no failure arm |
| `stoppedHere` | the **first** `notReached` stage that is preceded only by `reached`/`failed` stages — the boot's high-water mark |

Ordering is presentational, matching stage order in the catalog. Timing is **line
number and, when `[heartbeat] uptime_ms=` brackets the stage, guest-uptime
interpolation** — deliberately *not* wall-clock. `xtask` measures wall-clock deltas
from a live polling loop (`xtask/src/main.rs:686-688`), which is not re-derivable
from a stored serial; a post-hoc tool that printed wall-clock timings would be
inventing them. Line numbers are exact and reproducible. This is a real difference
from `boot-stages` and the UI labels the column `line` for that reason.

**Two honesty rules.** A stage marked `reached` on a run whose serial was truncated
mid-boot is still `reached` — the marker was observed. And the app does not render
a subsystem as "initialized" from the *`Initializing X...`* line alone; only the
terminator counts, because `Initializing` with no terminator is precisely the
signature of a boot that died inside that subsystem.

### 4.4 Multi-line and versioned records

**`FATAL_REGS`** is not a line, it is a record
(`kernel/src/arch_impl/aarch64/exception.rs:224-251`): a header line, then `x0…x30`
emitted four-per-line (`:239-247`), then `DISPATCH_TRACE cpu=<n>:` and a dumped
trace (`:248-251`). There are **two header shapes** — with `label=` (`:224`) and
without (`:295`, from `dump_el1_first_fault`) — and the parser must accept both.
The record ends at the first line that starts a new family or fails to match
`x<n>=<hex>`; a record that ends early is kept and flagged `truncated` rather
than discarded.

**`BXCAP`** decoding follows the schema's own rules
(`FAILURE-TRACE-CAPTURE-PLAN-2026-09-05.md` §4) rather than being lenient:
`BEGIN` without a matching `END` (same `seq`) ⇒ **`truncated`**, displayed as such;
an unknown major `v=` ⇒ **refuse the record and say so**, not a best-effort decode;
`seq` orders multiple captures and makes a nested capture visible as interleaving;
`sections_skipped` and per-row `q=` (`exact|racy|derived|unavail`) are surfaced in
the UI, because a `derived` row is not a measured one and the app must not present
it as though it were.

---

## 5. Launcher

### 5.1 arm: drive the gate scripts, do not re-issue their QEMU command line

**Decision: reuse the gate scripts.**

<!-- claim-lint:ok: the quoted sentence below is verbatim from
     docker/qemu/run-aarch64-boot-test-strict.sh:555-558, reproduced exactly
     as the script's own comment reads -->
The alternative — the Inspector composing `qemu-system-aarch64 …` itself — was
rejected on evidence. The strict gate's stop condition is *`score_serial` itself*
(`run-aarch64-boot-test-strict.sh:551-578`), and its comment records what happened
when that was not true: a narrower stop condition killed the VM at ~4.4 s, before
the futex handoff oracle (~5.8 s), the block-EINTR oracle, the strand census and
the injection oracle could be emitted, so *"the gate therefore killed the VM before
the evidence it scores could exist and failed every boot on 'marker missing',
including on main"*. A separately-maintained QEMU invocation in Swift would
reproduce that class of bug, and would also have to re-derive the `#528` no-NEON
preflight (`:180`), the `boot_tests` feature-profile guard (`:194-217`), and the
writable-ext2 copy (`:514-516`).

So `LocalGateLauncher` does exactly this:

```
1. mkdir  <store>/runs/<id>/gate-tmp
2. lock   docker/qemu/lib/qemu-host-lock.sh if present;
          else refuse if pgrep -x qemu-system-aarch64 is non-empty   (§1.6)
3. sample host facts  → manifest.host   (§5.3)
4. exec   env BREENIX_GATE_TMP=<store>/runs/<id>/gate-tmp \
              docker/qemu/run-aarch64-boot-test-strict.sh <boots>
          stdout+stderr → gate-stdout.txt, streamed to the CLI live
5. sample host facts again → manifest.host.end
6. harvest gate-tmp/**/serial*.txt into the run dir; parse; write manifest.json
7. verdict = .gateScript(argv, exitCode), text = gate-stdout.txt
```

<!-- claim-lint:ok: grep for BREENIX_GATE_TMP in each of the six gate
     scripts named in the §1.1 table (docker/qemu/run-aarch64-boot-test-strict.sh,
     run-aarch64-prod-profile-boot-test.sh, run-aarch64-testing-profile-boot-test.sh,
     run-x86-boot-tests.sh, run-x86-gate.sh, run-x86-prod-profile-boot-test.sh)
     shows the variable defined in each of the six -->
Step 4 is the whole integration, and it needs **no change to any gate script**,
because `BREENIX_GATE_TMP` already exists in all of them with an absolute-path guard
(§1.1). That is the design's cheapest and most important property.

The three arm profiles map to
`run-aarch64-boot-test-strict.sh` / `run-aarch64-prod-profile-boot-test.sh` /
`run-aarch64-testing-profile-boot-test.sh`. Because the strict gate requires a
`--features boot_tests` kernel and refuses otherwise with a marker census
(`:194-217`), the launcher surfaces that refusal as a first-class run state
`.refused(preflight)` — a run that did not boot, distinct from a run that failed.

### 5.2 x86: ssh to beast, run the gate in a private clone, pull evidence back

**Status: implemented in PR-5 (`RemoteCommand.swift`, `BeastLauncher.swift`), as
the mechanism below — corrected against beast, live, on 2026-09-06, from the
plan originally written here.** Two specifics in the original plan turned out
to be wrong for the `breenix-x86` Incus container specifically, verified by SSH
before writing the PR-5 code:

* **There is no `wrb` account in this container** (`id wrb` → "no such user").
  The repo lives at `/root/breenix`, owned by `root`, and commands in the
  container run as root: `sudo -n incus exec breenix-x86 -- bash -lc '<CMD>'`,
  with no `-iu` anything. A `sudo -iu wrb bash -lc '<CMD>'` shape is the
  pattern for a *different* beast container (`breeniac`, per global
  CLAUDE.md) and does not apply here.
* **`rsync -az beast:<clone>/...` cannot reach the evidence.** `<clone>` is a
  path inside the Incus container's own filesystem namespace, not a path on
  the beast host's filesystem an `ssh beast` + `rsync` pair can see. The
  working mechanism is a `tar` stream over the same ssh+incus-exec channel:
  `sudo -n incus exec breenix-x86 -- tar -czf - -C <clone> gate-tmp`, its
  stdout captured directly (kept separate from stderr, since it is raw gzip
  bytes) and extracted locally with `/usr/bin/tar -xzf`.

`run-x86-gate.sh:42-47` states the constraint that motivates having the
Inspector do this at all: the fetch/checkout of the branch under test *cannot*
live in the repo, because a script that `git reset --hard`s its own checkout
is a self-modification hazard. The Inspector is the thing outside the working
tree, so that step is its job.

```
1. ssh -T -o BatchMode=yes -o ConnectTimeout=15 beast '<CMD>' for each step below
   -- non-interactive throughout; each invocation runs sudo -n incus exec
   breenix-x86 -- ... as root, no login shell, no -iu anything
2. prepare: bash -lc 'git -C /root/breenix fetch origin && rm -rf <clone> &&
   git clone --shared /root/breenix <clone> && git -C <clone> checkout
   --detach <sha>' -- verified live: no SECOND fetch is needed inside the
   clone. `--shared`'s alternates file makes each of /root/breenix's objects
   (including one reachable only via refs/remotes/origin/* after step 1's
   fetch, not via any local branch) checkoutable in the new clone even
   though no ref in the new clone points at it yet.
3. run: bash -lc 'mkdir -p <clone>/gate-tmp && source /root/.cargo/env &&
   env BREENIX_GATE_TMP=<clone>/gate-tmp BREENIX_REPO_DIR=<clone>
   BREENIX_RUST_FORK=/root/breenix/rust-fork-real BREENIX_GATE_TIMEOUT=<n>
   <clone>/docker/qemu/run-x86-gate.sh <boots> <full|kthread>' -- the mkdir
   runs before the gate script so a build failure that does not reach the
   per-boot loop still leaves gate-tmp/ present for step 4
4. pull: tar -czf - -C <clone> gate-tmp   (stdout captured raw, not combined
   with stderr; written to a local .tar.gz, then extracted with
   /usr/bin/tar -xzf)
5. remove: rm -rf <clone>   -- runs via a `defer` registered once step 2
   succeeds, so it fires on the exit path after either a passing or a
   failing gate run; verdict is the gate script's own exit code from step 3
```

`BREENIX_RUST_FORK=/root/breenix/rust-fork-real` matters because
`rust-fork-real` is gitignored and untracked — a fresh clone has neither the
`rust-fork` symlink nor its target, which is exactly why `run-x86-gate.sh` has
its own repoint logic (`rm -f rust-fork; ln -s "$BREENIX_RUST_FORK" rust-fork`)
that this launcher relies on rather than duplicating.

A **private clone per run** rather than a shared checkout is required, not
preferred: it is the `[[workflow-worktree-isolation]]` rule (R83), and #797 is the
issue where concurrent lanes on this exact shared beast container clobbered each
other's `/tmp/breenix_gate_$i`. Setting `BREENIX_GATE_TMP` inside the clone closes
the other half. The clone path itself is a plain function of the run id
(`/root/breenix-<id>`, a sibling of the canonical checkout, matching the
`breenix-<lane>` naming convention the other beast clones on this host
already use) — not `mktemp`, which would make the launcher's plan-building
step impure and untestable as a pure function of (sha, boots, mode, paths).

**Deferred to a later PR: mid-run cancellation.** The design's original
intent — cancel by process group, not by name, so an interrupted `breenix-runs
run x86` cannot leave a stray remote QEMU boot running or degrade into
`pkill qemu-system-*` (`[[workflow-worktree-isolation]]` R84) — is not built in
PR-5. A `Ctrl-C` during the multi-minute `runGate` step today leaves the remote
gate (and its clone) running on beast; the operator cleans it up by hand
(`ssh beast 'sudo -n incus exec breenix-x86 -- rm -rf <clone>'`) until this
lands. This is a real, disclosed gap, not a silently dropped requirement.

### 5.3 Host facts the launcher samples itself

For runs the gate did not annotate (until #827 lands, plus each
`run.sh` and imported run), the Inspector records its own sample at start and at
end, and labels it as **its own** — kept separate from a `GATE_BOOT_FACTS` row, which
is the guest-annotated record:

| Field | How | Why |
|---|---|---|
| `wallStart` / `wallEnd` | `Date()`, stored UTC, displayed ET | the run's own duration |
| `qemuPeersStart` / `qemuPeersEnd` | `pgrep -c qemu-system-aarch64` (and `-x86_64`) | #826's unsampled confound: 4–6 concurrent QEMUs from unrelated worktrees |
| `loadavg1/5/15` | `sysctl -n vm.loadavg` | host contention |
| `qemuCPUSeconds` | `ps -o time= -p <pid>` sampled **before** the kill | starved vs wedged |
| `thermalPressure` | `pmset -g therm` (best-effort; `nil` when unavailable) | sustained-load throttling on this laptop |
| `hostModel`, `physMem`, `qemuVersion` | `sysctl hw.model hw.memsize`, `qemu-system-aarch64 --version` | cross-machine comparison |
| `gitSHA`, `gitDirty` | `git rev-parse HEAD`, `git status --porcelain` | which code produced this |

`clock_ratio` — guest uptime over host wall time, the starved-vs-wedged
discriminator — is computed **only** when the serial carries
`[heartbeat] … uptime_ms=` (`userspace/programs/src/heartbeat.rs:168`). Absent that,
the field is `nil` and the UI shows "—", not `1.0`.

---

## 6. PR plan

R157 small-PR mode: each lands the same day, does not reverse an earlier one, and
carries a test the operator can run. Each PR's test is `swift test` (and, where
noted, `cargo test`) — **no PR's acceptance depends on a QEMU boot**, which is what
keeps them same-day landable. PR-1 alone satisfies the operator's first ask.

| # | Title | Files | Test / oracle | Size |
|---|---|---|---|---|
| 1 | `breenix-runs run arm` + host-facts trace | `tools/breenix-runs/{Package.swift,Makefile,README.md}`, `Sources/BreenixRuns/{Store/{RunStore,RunManifest,RunIndex}.swift,Launch/{LocalGateLauncher,HostFacts,ProcessRunner}.swift}`, `Sources/breenix-runs/main.swift`, `Tests/…/{HostFactsTests,RunStoreTests}.swift`, `.gitignore` | `swift test`: `HostFacts` parses fixture strings for `pgrep -c` / `vm.loadavg` / `ps -o time=` (injected `ProcessRunner`, no real processes); `RunStore` round-trip = write manifest → rebuild index from scratch → identical run list; atomic-rename crash test leaves no partial manifest | M |
| 2 | Serial ingestion: scanner + marker families | `Sources/BreenixRuns/Parsing/{MarkerScanner,MarkerFamily,SerialIndex}.swift`, `Tests/Fixtures/*.txt`, `Tests/…/MarkerScannerTests.swift` | `swift test` against **real committed serials** copied into `Tests/Fixtures/`: from `slice3d/05-runtime-anti-vacuity-strict-serial.txt` assert `[BOOT_TESTS:PASS]` ×2 at lines 602 and 610, `TTBR0_ASID_CENSUS` ×2 with parsed counters, four distinct `EXEC_SMOKE` states, monotonic `heartbeat.uptime_ms`; **prefix-tolerance leg**: the literal `T2T3[BOOT_TESTS:` must match — a line-anchored regex reddens this test | M |
| 3 | Subsystem state machine + `show` / `facts` | `xtask/src/main.rs` (+`dump-boot-stages --json`), `tools/breenix-runs/Resources/boot-stages-{aarch64,x86_64}.json`, `Sources/BreenixRuns/Subsystems/{StageCatalog,StateMachine}.swift`, `Sources/breenix-runs/` subcommands, `tests/boot_stage_catalog_export.rs` | `cargo test`: committed JSON matches the live catalog, **census-anchored on entry count + name set** (deleting a `BootStage` reddens it; the assertion contains no literal stage-name list). `swift test`: green strict serial ⇒ each arm64 kernel stage `reached`; a preserved failure serial ⇒ exactly one `stoppedHere` with the correct predecessor | M |
| 4 | Import existing evidence | `Sources/BreenixRuns/Store/Importer.swift`, `Sources/breenix-runs/import.swift`, `Tests/…/ImporterTests.swift` | `swift test`: import a synthesized gate-tmp tree + a preserved-failures dir + a loose serials dir into a temp store; assert arch/profile inference per §3.3, `verdict == .unknown` for loose serials (**not invented**), and that a second import is a no-op (identical ids, unchanged count) | M |
| 5 | x86 beast launcher | `Sources/BreenixRuns/Launch/{BeastLauncher,RemoteCommand}.swift`, `Sources/breenix-runs/` (`run x86`, `--dry-run`), `Tests/…/BeastLauncherTests.swift` | `swift test`: the launcher's argv is a **pure function** of (sha, boots, mode, paths) — snapshot-assert the exact `ssh`/`incus exec`/`rsync` argv, that `BREENIX_GATE_TMP` points inside the per-run clone, and that the teardown removes the clone. No network in the test. `breenix-runs run x86 --dry-run` prints the same plan for the operator | M |
| 6 | SwiftUI app + `make app` | `Sources/BreenixRunInspector/**` (App, Sidebar, Subsystems, Messages panes), `Makefile` (`app` target), `Sources/BreenixRuns/ViewModels/*` | `swift build --target BreenixRunInspector`; `swift test` on the view models (sidebar sorts newest-first incl. the amber attributed state; message filter predicate selects only the intended families); `make app && plutil -lint "Breenix Run Inspector.app/Contents/Info.plist"` exits 0 | L |
| 7 | Traces pane: host facts, BXCAP, FATAL_REGS | `Sources/BreenixRuns/Parsing/{BXCAPDecoder,FatalRegsDecoder,BootFactsParser}.swift`, `Sources/BreenixRunInspector/TracesPane.swift`, `Tests/…/{BXCAPTests,FatalRegsTests}.swift` | `swift test`: `FATAL_REGS` assembled from a **real committed fatal serial** (`docs/planning/teardown-unification/607-576-serials/gate-clean100-cortexa72-boot3-stackpc-8600000e.txt`), both header shapes (with and without `label=`), x0…x30 grid complete; BXCAP legs — `BEGIN` w/o `END` ⇒ `truncated`, `v=2` ⇒ **refused not decoded**, `seq` interleaving ⇒ two captures; `GATE_BOOT_FACTS` parser accepts an **unknown extra key** without failing, and a serial with no such record yields the explicit "not present" state | M |
| 8 | Compare view + `tail` | `Sources/BreenixRuns/Diff/RunDiff.swift`, `Sources/BreenixRunInspector/ComparePane.swift`, `Sources/breenix-runs/tail.swift` | `swift test`: diff of two fixture runs reports the exact subsystem-state delta and marker-count delta, and is empty for a run against itself; `tail` follows a file being appended to in a temp dir and terminates on EOF+exit | S |

Ordering rationale: 1 gives the operator the CLI immediately; 2–4 build the reader
on committed fixtures with no boots; 5 adds the second host; 6–8 are the app. Each
of 2, 3, 4, 7 is independently useful before the app exists, because the CLI
consumes them.

---

## 7. Open questions

1. **`GATE_BOOT_FACTS` final field set and carrier.** The task states the
   colon-delimited bracketed form; the failure-capture plan's PR-1 describes a
   space-delimited `.facts` sidecar with a partly different field set
   (`qemu_peers_start` vs `qemu_at_start`, plus `clock_ratio`). The Inspector parses
   generically (§4.2) so either lands cleanly, but **which is canonical, and does the
   sidecar coexist with the serial line?** Worth one answer before PR-7.
2. **Should the Inspector own the aarch64 host lock, or only consume it?** If
   `qemu-host-lock.sh` is advisory, an operator running `./run.sh` by hand still
   perturbs an Inspector run. Consuming it is planned; owning a broader policy is not.
3. **Retention.** Serials are up to ~1.7 MB and a 20-boot strict run produces 20.
   Cap total store size, age runs out, or keep everything and let the operator prune?
   Default proposed: keep everything, show store size, offer `breenix-runs prune`.
4. **Should `run.sh` sessions be captured?** They are the operator's most frequent
   runs but produce no scored verdict and write to `mon:stdio`. Capturing them means
   a `--tee` wrapper. Deferred; not in the PR plan.
5. **Does the operator want the app to *launch* runs, or only read them?** The plan
   makes launching CLI-only (the app reads the store and can `tail` a live run).
   Adding a launch button is small but changes the app's permission surface.
6. **Committed fixture size.** PR-2 and PR-7 commit real serials into
   `Tests/Fixtures/`. Full files are up to 1.7 MB; excerpts are smaller but are no
   longer byte-identical evidence. Proposed: commit whole files for the two primary
   fixtures, excerpts for the rest, and say which is which in the test.
7. **`docs/planning/**/serials/` import default.** 2 725 files, 317 MB of
   `docs/planning`. Import on first launch by default, or only on explicit request?
   Proposed: explicit, with a first-run prompt.

---

## 8. What is NOT claimed

* **The Inspector does not score runs.** Each gate verdict is the gate script's own
  stdout and exit status (§1.2). Swift parsing drives display and navigation only. A
  run whose verdict was not recorded shows `.unknown`, not a computed guess.
* **No x86 execution on this Mac.** Each x86 run is dispatched to beast
  (`[[beast-x86-build-host]]`). `breenix-runs run x86` on a machine that cannot reach
  beast fails with that message; it does not fall back to local TCG.
* **Parallels runs are out of scope for v1.** `run.sh --parallels` writes a fixed
  `/tmp/breenix-parallels-serial.log` that accumulates across boots
  (`run.sh:201`; CLAUDE.md's mandatory restart protocol exists because of it), needs
  `prlctl` VM-state management, and cannot be captured by the `BREENIX_GATE_TMP`
  mechanism that makes the QEMU path cheap. Importing a Parallels serial by hand
  works; launching one does not.
* **No live streaming from the guest.** Ingestion is post-hoc over serial files.
  `tail` follows a file being written; it does not attach to QMP, GDB, or the trace
  ring. The app does not claim to show a running kernel's state — only what has
  been written to serial.
* **No GDB or QMP integration.** `breenix-gdb-chat/` and
  `scripts/forensic-capture.sh` remain separate tools. Notably
  `forensic-capture.sh` requires `/tmp/breenix-qmp.sock`, **which no gate provides**
  — wiring that up is the failure-capture lane's PR-6, not this one.
* **No decoding of raw trace-buffer memory dumps.** `scripts/trace_memory_dump.py`
  owns that. The Inspector renders `[BXCAP:EV …]` *serial* records; it does not parse
  `trace_buffers.bin`.
* **The per-CPU timeline is not a total order.** Per-CPU slot order is
  authoritative; the merged view is labelled approximate, per the BXCAP schema's own
  exclusion and `trace_memory_dump.py`'s documented out-of-order commits.
* **BUILD_ID is aarch64-only.** `BREENIX_BUILD_ID` has exactly one consumer
  (`kernel/src/main_aarch64.rs:488`). x86 runs show "no BUILD_ID banner on this arch".
* **Wall-clock per-stage timings are not reproduced.** The Inspector reports line
  numbers and guest uptime; `xtask boot-stages`' wall-clock deltas come from a live
  polling loop and are not re-derivable post-hoc (§4.3).
* **Neither `GATE_BOOT_FACTS` nor `BXCAP` exists at `7a19f550`.** Verified by grep
  over the whole worktree. Each feature reading them must degrade to an explicit
  "not present" state, and the plan is written on the assumption that they may land
  in a different shape than described.
* **No CI.** This repo has no GitHub Actions; "the test you can run" means a command
  the operator runs locally.
