# The effort path

`path.json` is the plan for Breenix: an ordered list of efforts that together make a POSIX kernel with a
userspace you can build on. Each effort has an outcome and a set of milestones. Vigil's Efforts page reads this
file from main and shows each milestone's state from live evidence; nothing in it is a rating someone sets.

## Efforts

    { "id", "title", "outcome",
      "suite":      the effort's suite id (docs/suites/<id>.json), when it has one,
      "area":       the POSIX area its system interfaces belong to, as Vigil's syscall coverage names it,
      "subsystems": issue/merge taxonomy ids whose open issues and recent merges belong to this effort,
      "targets":    the targets the effort is measured on, when not all four (see below),
      "targetsNote": one line on why a target is missing and when it joins, only beside "targets",
      "milestones": [ { "id", "title", "measure" } ] }

## Targets

<!-- claim-lint:ok: tests/suite_manifests.rs checks the target sets and the guard table is in tests/effort_target_guard_test.py. -->
The main-path targets are `arm64-qemu` (ARM64 QEMU), `parallels`, `vmware` and `x86` (x86-64), the ids
`scripts/effort-target-guard.py` uses for `path.json`. An effort without `targets` is measured on those four. With it, a
non-empty list in that order without repeats, the effort is measured only on the targets listed: wherever a
measure below says "every target", it means those. A target left out neither counts toward the effort nor holds
it back. An effort without `x86` measures `interfaces` on ARM64 only, and has no `profiles` milestones.

`gpu`, `breengel`, `window-manager`, `ui-testing` and `gui-apps` list `arm64-qemu`, `parallels` and `vmware`:
the x86-64 gate host is VGA only, so x86-64 is measured up to the shell, and it joins the GPU-era efforts when
the dedicated Breenix hardware box replaces that host.

`x86-metal` is a physical x86-64 server reached by UEFI HTTP boot, measured only by the efforts in
`metal.json`. Vigil's platform is `metal` ("x86-64 metal"). An omitted target list still means the four
main targets above; it does not include metal. The main and metal paths advance independently.

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

- `{"bootPath": "<milestone id>"}`: the docs/boot-path.json milestone passes in the newest boot on every target
  (ARM64 QEMU, Parallels, VMware, x86-64). On x86-64 only the full tests gate on the default PC counts.
- `{"profiles": "<milestone id>"}`: that boot-path milestone passes in the newest full x86-64 gate on every
  hardware profile in docs/x86-profiles.json.
- `{"suite": "<id>", "categories": ["<category id>", ...]}`: every case in those categories of that suite
  passes. It is done when it passes on every target (ARM64 QEMU, Parallels, VMware, x86-64); until the suite
  has those categories the milestone is shown as not measured yet, which is the work of writing its cases.
- `{"interfaces": "<area>"}` or `{"interfaces": ["name", ...]}`: those POSIX system interfaces are backed by a
  dispatched syscall on both architectures.

Ids are lowercase words joined by `-`. Add an effort or milestone by editing this file; Vigil picks it up.
