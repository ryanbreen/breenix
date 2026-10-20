# The effort path

`path.json` is the plan for Breenix: an ordered list of efforts that together make a POSIX kernel with a
userspace you can build on. Each effort has an outcome and a set of milestones. Vigil's Efforts page reads this
file from main and shows each milestone's state from live evidence; nothing in it is a rating someone sets.

## Efforts

    { "id", "title", "outcome",
      "suite":      the effort's suite id (docs/suites/<id>.json), when it has one,
      "area":       the POSIX area its system interfaces belong to, as Vigil's syscall coverage names it,
      "subsystems": issue/merge taxonomy ids whose open issues and recent merges belong to this effort,
      "targets":    the targets the effort is measured on, explicitly listed (see below),
      "needsGpu":   true when the effort requires a GPU (omit otherwise),
      "targetsNote": one line on the target policy, only beside "targets",
      "milestones": [ { "id", "title", "measure" } ] }

`needsGpu` declares a GPU requirement for Vigil to use when filtering efforts;
repository runners do not act on this field. The GPU-less `x86-metal` target
(Vigil platform `metal`) follows the five efforts in `metal.json`.
The field is a boolean; only `true` is written, and absence means no GPU requirement.

## Targets

As of 2026-10-18 (S23), the main-path targets are `arm64-qemu` (ARM64 QEMU),
`parallels` and `vmware`. Each effort lists its targets explicitly; an already
narrower effort retains its narrower list without x86. A target left out does not
count toward or block the effort. `interfaces` is measured on ARM64 on this path.
x86 hardware-profile milestones remain defined but are outside measurement on the main path.

x86-64 requires evidence from x86-metal using the [metal boot loop](../metal/README.md),
measured by `metal.json`. Vigil's platform is `metal` ("x86-64 metal").

A `path.json` effort that x86-64 leads on bare metal adds `x86-metal` at the end of its list
(operator decision D6, 2026-10-20): `server`, `users`, `filesystem` and `toolchain`
([the server plan](../metal/server-plan.md)). Their lanes are accepted on the R240 first, then the
catch-up brings ARM64 QEMU, Parallels and VMware to the same bar. An effort measured on
`x86-metal` alone belongs in `metal.json`.
The main and metal paths advance independently. [S23's execution policy](../x86-execution-policy.md)
supersedes earlier x86 QEMU gate and landing requirements.

`metal.json` uses the same effort schema with exactly `["x86-metal"]` on its efforts. Its `bootPath`
measures resolve against `docs/metal/boot-path.json`; suite measures use the existing manifests.
Storage is scheduled after `metal-userspace`, in parallel with `metal-network`. The linear display
order keeps networking first, followed by storage, so Vigil continues to show networking as the
current step; this order is not a dependency between them. SATA remains enabled in AHCI mode,
and storage includes disk writes and the Breenix filesystem. Earlier boots use the RAM-disk root
and do not depend on the disk. The top-level `later` list records unscheduled work and is not
part of the effort sequence; link aggregation follows networking when scheduled.

### Metal definitions and evidence

The metal boot path uses production x86-64 and `scoring: "modes"`: select a mode's list, or the
`x86_64` list of a `kernel: true` milestone. Mode stage names resolve against `gateArch`'s catalog.
Its modes are Boot (`default`), Shell, Probe and Suite. `{name, host}` stages require host events,
not serial substrings. Inline `{name, marker}` stages use serial markers; `match: "line"` requires
an exact whole-line result so echoed shell input cannot satisfy the console computation.

Delivery and first-light define the initial contract. Later boot milestones carry `stub: true` and
proposed, non-empty stage lists; their producing lanes finalize the markers with their code and
remove the stub flag. A stub is a definition under construction, not a completion claim.
The `firsts` array names stages to celebrate from a finished valid capture; a first with `suite`
also requires the complete passing manifest verdict, beyond the named DONE stage.

Metal evidence comes from finished, clean main-code boots on platform `metal`, scored against that
boot's saved definition, with no console gap or interrupted/incomplete capture. Lane boots show work
in progress. Metal evidence does not complete main-path efforts, nor does main-path evidence complete
metal efforts. The kernel reads both boot paths only to protect serial marker lines from log drops.

### Effort-target policy

ARM64 QEMU remains available for development. Main-path lane history on `x86`, `parallels` or `vmware`
requires catch-up; metal lanes may use those targets for landing checks. Main-path lane history is
refused on `x86-metal` even with catch-up, except a lane of an effort whose targets list `x86-metal`.
For a metal boot, commits outside `origin/main` must also be first-parent reachable from a `metal-*`
lane on origin (including `metal-stream/definition`) or from such an effort's lane on origin. Keep
those refs until the work reaches main: a second-parent merge from an unrelated branch is refused
even if that branch is later renamed or deleted. A main or metal second parent is allowed when its
commits meet that ancestry rule. The guard reads origin live and refuses when origin is unavailable.

Cherry-picked or retyped main-path work has new ancestry that the guard cannot identify. Metal PRs
must list merge parents and files outside metal paths for review of that boundary.

## Measures

A milestone has exactly one measure:

- `{"bootPath": "<milestone id>"}`: the docs/boot-path.json milestone passes in the newest boot on the targets
  listed by the effort (ARM64 QEMU, Parallels and VMware).
- `{"profiles": "<milestone id>"}`: a retained x86 hardware-profile definition from docs/x86-profiles.json,
  outside measurement on the main path under S23.
- `{"suite": "<id>", "categories": ["<category id>", ...]}`: the cases in those categories of that suite
  must pass. It is done when it passes on the targets listed by the effort (ARM64 QEMU, Parallels and VMware); until the suite
  has cases in each named category the milestone is not measured yet, even if the populated categories pass.
  A planned category with no cases blocks completion on each target and therefore blocks the milestone.
  Scorers and tools must check category presence and non-empty case lists before summing case results;
  missing or empty categories cannot be omitted from completion requirements. This rule applies to both effort paths.
  Full-suite acceptance also requires cases in each manifest category (`scripts/suite-verdict.py`, shared by
  `scripts/suite-sequence.py` and the boot watchers). A selected case or category run is incremental evidence,
  not full-suite or milestone completion.
- `{"interfaces": "<area>"}` or `{"interfaces": ["name", ...]}`: those POSIX system interfaces are backed by a
  dispatched syscall on the architectures in the effort's target list.

Ids are lowercase words joined by `-`. Add an effort or milestone by editing this file; Vigil picks it up.

Profile milestones on efforts without x86 are retained as out of scope: Vigil's existing `BreenixPath.swift` defines `outOfScope` as excluded from progress and non-blocking, and excludes those milestones from the effort count. See `BreenixPath.swift:56-59,168-170,232-234`.
