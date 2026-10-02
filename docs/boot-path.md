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

`scripts/boot-interactive.sh --mode MODE` boots the same disk six ways (`modes` in
`boot-path.json`):

- `tests` (default): the testing kernel and its test loader. This is the gate above.
- `probe`: the production kernel runs `/sbin/probe` as PID 1, which checks one
  subsystem per line (`PROBE <id> OK|FAIL ...`) and ends with `PROBE DONE`.
- `shell`: the production kernel runs `/sbin/init shell`, a bsh prompt on the serial
  console you can type into.
- `desktop`: the production kernel runs `/sbin/init desktop`, the window manager and a
  terminal in the VM window (`--no-display` keeps it headless).
- `program`: the production kernel runs `/sbin/probe --run PATH` as PID 1 (`--program PATH`,
  an absolute guest path, passed as `-fw_cfg name=opt/breenix/program,string=PATH`). Probe
  runs that one program, relays its output, and prints `RUN PATH START` once the program has
  been exec'd, `RUN PATH EXIT <code>` (or `SIGNAL <n>`) and `RUN PATH DONE PASS|FAIL <reason>`;
  a program that never starts gets only the `DONE FAIL` line. It passes when the program exits
  0 and printed no line containing `FAIL`; one running past 60 seconds is killed and fails.
  Stage markers are substrings and the program's output is relayed unchanged, so a program
  that prints probe's own `START` or `DONE PASS` text can match those stages.
- `suite`: the production kernel runs an effort suite, `/sbin/suite-ID`, as PID 1
  (`--suite ID`, passed as `-fw_cfg name=opt/breenix/suite,string=ID`). The suite runs every
  case of `docs/suites/ID.json`, prints `SUITE ID START`, one `SUITE ID CASE` line per case and
  `SUITE ID DONE passed=P failed=F skipped=S total=N`, and leaves its scored panel on screen
  (`docs/suites/README.md`). Parallels and VMware boot a suite with
  `./run.sh --parallels|--vmware --suite ID`, and the x86-64 gate with
  `BREENIX_BOOT_SUITE=ID docker/qemu/run-x86-gate.sh`: both write `/etc/breenix/boot-target`
  (one line, `suite ID`) onto a copy of the ext2 disk. The kernel takes the fw_cfg mode
  first (x86-64 QEMU reads fw_cfg too, for `default` and `suite`), then that file, else
  `default`. Suite mode exercises the kernel milestones only.

The non-test modes build the kernel with no features, exactly like the prod-profile gate,
and pass the mode to it with `-fw_cfg name=opt/breenix/mode,string=MODE`. The kernel
prints `[boot] Boot mode: MODE` for the mode that runs (`program PATH` in program mode,
`suite ID` in suite mode):
`default` when none is given, which runs `/sbin/init` with no arguments, as before. An
unknown mode, any mode given to the testing kernel, program mode without an absolute
program path, suite mode without a valid suite id, an unusable boot-target file, or probe,
program or suite mode when its binary is missing, is not a loadable ELF for the kernel's
architecture or fails to start also runs `default`, and the kernel
notes the ignored request on its own line. A mode's stages for a milestone are `stages[MODE]` when present, else
`stages["aarch64"]` when the milestone has `"kernel": true`; otherwise the mode does not
exercise that milestone.

## Watching a boot

The VM screen shows the boot, not the log. As soon as the framebuffer exists the kernel
draws a boot screen: "Breenix", the boot mode, a progress bar, and the kernel stages of
milestones 1-5 ticking in, ending in "Starting PID 1" with the program it starts. A
testing kernel shows "Loading test programs N of M: NAME" while its loader runs, so a hang
names the program it stopped on. When PID 1 draws for the first time the screen is
handed over to it: `/sbin/probe` draws its checklist, `probe --run` its program panel
(output, elapsed time, verdict). If PID 1 never draws, the boot screen stays up. A kernel
panic or fatal EL1 fault replaces whatever is on screen with a red diagnostics screen: the
message or fault, the boot stage, and the last 20 log lines. Serial output is unchanged;
every log line still goes there.

- `--display` opens the screen in a window.
- `--qmp SOCKET` opens a QMP socket; `scripts/qmp-screendump.py SOCKET out.png` saves the
  screen at that moment, with or without `--display`.
- `--fbconsole log` draws the kernel log on screen instead of the boot screen (kernel lines
  only, not userspace output).

Wait for a serial line rather than a fixed time, since the build before the boot takes a
minute or two. Remove the old log first: the script truncates it only once the build is done.

```bash
log="$TMPDIR/breenix-boot/serial.txt"; rm -f "$log"
scripts/boot-interactive.sh --mode program --program /usr/local/test/bin/argv_test \
    --qmp "$TMPDIR/breenix.qmp" --serial-log "$log" --idle-exit 90 < /dev/null &
until grep -q ' DONE ' "$log" 2>/dev/null; do sleep 2; done
scripts/qmp-screendump.py "$TMPDIR/breenix.qmp" "$TMPDIR/screen.png"
```

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
