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
      "targetsNote": one line on the target policy, only beside "targets",
      "milestones": [ { "id", "title", "measure" } ] }

## Targets

<!-- claim-lint:ok: docs/x86-execution-policy.md records S23 (operator, 2026-10-18); this paragraph prescribes policy, not a measured result. -->
As of 2026-10-18 (S23), the main-path targets are `arm64-qemu` (ARM64 QEMU),
`parallels` and `vmware`. Each effort lists its targets explicitly; an already
narrower effort retains its narrower list without x86. A target left out does not
count toward or block the effort. `interfaces` is measured on ARM64 on this path.
x86 hardware-profile milestones remain defined but are measured nowhere on the main path.

<!-- claim-lint:ok: docs/x86-execution-policy.md records S23 (operator, 2026-10-18); this paragraph prescribes policy, not a measured result. -->
x86-64 is proved on the R240 through the [metal boot loop](../metal/README.md),
measured by `metal.json`. Vigil's platform is `metal` ("x86-64 metal").
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
refused on `x86-metal` even with catch-up. For a metal boot, commits outside `origin/main` must also
be first-parent reachable from a `metal-*` lane on origin (including `metal-stream/definition`). Keep
those refs until the work reaches main: a second-parent merge from an unrelated branch is refused
even if that branch is later renamed or deleted. A main or metal second parent is allowed when its
commits meet that ancestry rule. The guard reads origin live and refuses when origin is unavailable.

Cherry-picked or retyped main-path work has new ancestry that the guard cannot identify. Metal PRs
must list merge parents and files outside metal paths for review of that boundary.

## Measures

A milestone has exactly one measure:

<!-- claim-lint:ok: docs/x86-execution-policy.md records S23 (operator, 2026-10-18); this paragraph prescribes policy, not a measured result. -->
- `{"bootPath": "<milestone id>"}`: the docs/boot-path.json milestone passes in the newest boot on every target
  listed by the effort (ARM64 QEMU, Parallels and VMware).
<!-- claim-lint:ok: docs/x86-execution-policy.md records S23 (operator, 2026-10-18); this paragraph prescribes policy, not a measured result. -->
- `{"profiles": "<milestone id>"}`: a retained x86 hardware-profile definition from docs/x86-profiles.json,
  measured nowhere on the main path under S23.
<!-- claim-lint:ok: docs/x86-execution-policy.md records S23 (operator, 2026-10-18); this paragraph prescribes policy, not a measured result. -->
- `{"suite": "<id>", "categories": ["<category id>", ...]}`: every case in those categories of that suite
  passes. It is done when it passes on every target listed by the effort (ARM64 QEMU, Parallels and VMware); until the suite
  has those categories the milestone is shown as not measured yet, which is the work of writing its cases.
- `{"interfaces": "<area>"}` or `{"interfaces": ["name", ...]}`: those POSIX system interfaces are backed by a
  dispatched syscall on the architectures in the effort's target list.

Ids are lowercase words joined by `-`. Add an effort or milestone by editing this file; Vigil picks it up.
