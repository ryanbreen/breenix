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

The targets are `arm64-qemu` (ARM64 QEMU), `parallels`, `vmware` and `x86` (x86-64), the ids
`scripts/effort-target-guard.py` uses. An effort without `targets` is measured on all four. With it, a
non-empty list in that order without repeats, the effort is measured only on the targets listed: wherever a
measure below says "every target", it means those. A target left out neither counts toward the effort nor holds
it back. An effort without `x86` measures `interfaces` on ARM64 only, and has no `profiles` milestones.

`gpu`, `breengel`, `window-manager`, `ui-testing` and `gui-apps` list `arm64-qemu`, `parallels` and `vmware`:
the x86-64 gate host is VGA only, so x86-64 is measured up to the shell, and it joins the GPU-era efforts when
the dedicated Breenix hardware box replaces that host.

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
