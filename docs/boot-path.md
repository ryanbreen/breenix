# The boot path

Breenix is brought up one milestone at a time, in the order a boot exercises it: arch
bring-up, interrupts and time, devices, filesystems, the scheduler, the first userspace
process, then process lifecycle, signals and IPC, userspace filesystem work, the TTY and
shell, networking, and runtimes. `docs/boot-path.json` lists the milestones and, for
each arch, which stages of the boot-stage catalog (`xtask/src/boot_stages.rs`) its gate
needs. The catalog stays the only place a stage's serial marker, failure meaning and
check hint are written down; the milestone file names stages and adds a marker only
where the catalog has none.

## The gate

A milestone passes when **one** `--features testing` boot on ARM64 prints every one of
its stages, with no kernel panic, soft lockup or EL1 abort (the `fatal` patterns). That
is the whole gate:

```bash
scripts/boot-interactive.sh --serial-log "$TMPDIR/breenix-boot/serial.txt" --idle-exit 30 < /dev/null
```

It builds userspace, the ext2 disk and the testing kernel, then boots once with the serial
console on the terminal and in the log file (Ctrl-A X quits; `--idle-exit` stops a boot that
has gone quiet; `--display` also opens the VM's screen). From a fresh worktree that is about a
minute and a half. Run from a terminal, the same script is an interactive boot you can type into. x86-64 is scored against its own
stage list and shown alongside, but does not block moving on.

## Boot modes

`scripts/boot-interactive.sh --mode MODE` boots the same disk four ways (`modes` in
`boot-path.json`):

- `tests` (default): the testing kernel and its test loader. This is the gate above.
- `probe`: the production kernel runs `/sbin/probe` as PID 1, which checks one
  subsystem per line (`PROBE <id> OK|FAIL ...`) and ends with `PROBE DONE`.
- `shell`: the production kernel runs `/sbin/init shell`, a bsh prompt on the serial
  console you can type into.
- `desktop`: the production kernel runs `/sbin/init desktop`, the window manager and a
  terminal in the VM window (`--no-display` keeps it headless).

The non-test modes build the kernel with no features, exactly like the prod-profile gate,
and pass the mode to it with `-fw_cfg name=opt/breenix/mode,string=MODE`. The kernel
prints `[boot] Boot mode: MODE` for the mode that runs: `default` when none is given,
which runs `/sbin/init` with no arguments, as before. An unknown mode, or any mode given
to the testing kernel, also runs `default`, and the kernel notes the ignored request on
its own line. A mode's stages for a milestone are `stages[MODE]` when
present, else `stages["aarch64"]` when the milestone has `"kernel": true`; otherwise the
mode does not exercise that milestone.

## Focus and backtracking

The focus is the earliest milestone whose gate does not pass. Because a boot runs every
earlier stage on its way, every boot re-checks every earlier gate for free: if a change
breaks an earlier milestone, that milestone stops passing and becomes the focus again.
Nothing else tracks regressions.

## How to work a milestone

- Fix the first stage that does not appear. Its failure meaning and check hint in the
  catalog say where to look; the last lines of the serial say where the boot stopped.
- Prove the fix with one build and one boot, and read the serial.
- Do not add soak runs, repeated-boot batteries, ratchets, oracles, census lines or
  evidence documents to pass a gate. Repeating a boot is for chasing a specific flake,
  never for passing.
- When a milestone needs a stage the catalog does not have, add the marker to the kernel
  and the catalog, and name it in `boot-path.json`.

Vigil's Breenix page reads this file, runs boots, and shows the ladder live.
