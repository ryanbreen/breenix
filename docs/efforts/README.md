# The effort path

`path.json` is the plan for Breenix: an ordered list of efforts that together make a POSIX kernel with a
userspace you can build on. Each effort has an outcome and a set of milestones. Vigil's Efforts page reads this
file from main and shows each milestone's state from live evidence; nothing in it is a rating someone sets.

## Efforts

    { "id", "title", "outcome",
      "suite":      the effort's suite id (docs/suites/<id>.json), when it has one,
      "area":       the POSIX area its system interfaces belong to, as Vigil's syscall coverage names it,
      "subsystems": issue/merge taxonomy ids whose open issues and recent merges belong to this effort,
      "milestones": [ { "id", "title", "measure" } ] }

## Measures

A milestone has exactly one measure:

- `{"bootPath": "<milestone id>"}`: the docs/boot-path.json milestone passes in the latest gate boot.
- `{"suite": "<id>", "categories": ["<category id>", ...]}`: every case in those categories of that suite
  passes. It is done when it passes on every target (ARM64 QEMU, Parallels, VMware, x86-64); until the suite
  has those categories the milestone is shown as not measured yet, which is the work of writing its cases.
- `{"interfaces": "<area>"}` or `{"interfaces": ["name", ...]}`: those POSIX system interfaces are backed by a
  dispatched syscall on both architectures.

Ids are lowercase words joined by `-`. Add an effort or milestone by editing this file; Vigil picks it up.
