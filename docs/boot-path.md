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
  Parallels and VMware boot the testing kernel with `./run.sh --parallels --tests` and
  `./run.sh --vmware --tests`, which wait for the kernel's `USERSPACE TEST REPORT DONE`
  line (or fatal output, or the `--gate-timeout` deadline), then stop the VM.
- `probe`: the production kernel runs `/sbin/probe` as PID 1, which checks one
  subsystem per line (`PROBE <id> OK|FAIL ...`) and ends with `PROBE DONE`.
  Parallels and VMware run it with `./run.sh --parallels --probe` and
  `./run.sh --vmware --probe`: the same production kernel selects `/sbin/probe`
  from a boot-target file containing `probe` on a copy of the ext2 disk. These
  runners request Vigil mode `probe` ([vigil#2](https://github.com/ryanbreen/vigil/issues/2)
  tracks the constructor overriding it to `default`), wait for `PROBE DONE`, stop
  the VM, and exit nonzero for a missing completion or `failed>0`.
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
  (`--suite ID`, passed as `-fw_cfg name=opt/breenix/suite,string=ID`). The suite runs the
  case of `docs/suites/ID.json`, prints `SUITE ID START`, one `SUITE ID CASE` line per case and
  `SUITE ID DONE passed=P failed=F skipped=S total=N`, and leaves its scored panel on screen
  (`docs/suites/README.md`). Parallels and VMware boot a suite with
  `./run.sh --parallels|--vmware --suite ID`, and the x86-64 gate with
  `BREENIX_BOOT_SUITE=ID docker/qemu/run-x86-gate.sh`: both write `/etc/breenix/boot-target`
  (one line, `suite ID`) onto a copy of the ext2 disk. The kernel takes the fw_cfg mode
  first (x86-64 QEMU reads fw_cfg too, for `default` and `suite`), then that file, else
  `default`. Suite mode exercises the kernel milestones and the first userspace process: the
  suite's `START` and `DONE` lines show PID 1 running in user mode and making syscalls.
  ARM64 QEMU waits two seconds after a complete `DONE` line for rendering, then
  holds the scored panel for `BREENIX_SUITE_HOLD` seconds (default 5), continuing
  to watch for fatal output. It stops the VM, validates the records against the
  manifest and releases the shared boot slot. A missing DONE or fatal kernel
  output fails the boot; its idle exit and `--gate-timeout` DONE deadline also
  stop unfinished suites. Parallels and VMware retain their scored panel until
  Ctrl-C stops the VM. `--suite --test` on Parallels
  waits for DONE before its timed screenshot and exit; VMware rejects `--test`.
  `--gate-timeout N` sets the DONE deadline (default 1800 seconds for ARM64 QEMU
  suites and Parallels/VMware suites and probes); QEMU enforces it even when
  `--idle-exit 0` disables idle shutdown.
  The `default` boot (what Parallels and VMware run without `--suite` or `--probe`) measures that
  milestone by init's `[init] Breenix init starting (PID 1)` and `[init] Boot script completed`.

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

The scheduler milestone's last stage, "User work ran on every online CPU", is the testing
kernel's: once its userspace has finished, the kernel prints each online CPU's count of
user-thread dispatches (`[smp] user-thread dispatches per CPU: cpu0=N ...`) and the stage
marker only when more than one CPU is online and every one of them dispatched a user
thread. The other modes therefore list the scheduler milestone's stages themselves, without
it; the `suite` list names markers for both arches, since x86-64 suite boots are scored
against it too.

## Shared host slots

Use `docker/qemu/run-x86-gate.sh` for queued x86 builds and boots: the helper
provides two build leases and one boot lease. Run Inspector installs its current
helper outside the tested checkout, including when testing an older revision.
Historical gates hold both leases for their whole run. Launchers sourcing
`docker/qemu/lib/qemu-host-lock.sh` also enroll in the host queue; on Linux they
hold a build lease until boot admission. Mac builds remain unrestricted.

Use `scripts/boot-interactive.sh` or `run.sh` for a Mac boot. The shared helper
queues QEMU, Parallels and VMware behind one Mac lease. VM deployment and the
suite panel are part of that lifetime. Stop the run to stop its VM and free the
lease. An unqueued running, paused or suspended Breenix VM must be stopped by its
owner before a queued deployment can proceed; launchers must not stop or delete
other lanes' VMs by name pattern.

Keep the permanent lease files under `/tmp/breenix-host-slots`; do not remove
them while runs are alive. `metadata.lock` protects holder publication and FIFO
wait tickets. A worker owns the leases separately from the foreground run handle.
If that handle is killed, the worker stops its descendants and registered VM
before releasing the leases. Catchable signals allow up to 120 seconds for the
runner's cleanup; preserve its own exit status. An unqueued QEMU also blocks
boot admission, including after a worker itself dies.

Expect wait messages immediately and once a minute, identifying the holder or
an earlier queued request. Queue context belongs in `*.host-slots.jsonl` sidecars,
never guest serial inputs. Build logs belong beside the gate output. Create a
fresh private Cargo home with the shared configuration and credentials; download
its dependencies rather than copying a registry that another Cargo can mutate.
The worker removes private homes during cleanup, including after a killed handle.

For one deliberately unqueued **manual Mac boot**, prefix the command with
`BREENIX_BOOT_NO_QUEUE=1`. Keep it scoped to that command. Retain the legacy
per-binary QEMU lock even for a bypass; VM cleanup remains limited to the run's
own registered VM. x86 and VMware gates must ignore this variable.

Run the helper tests without a VM with `python3 tests/host_slots_test.py`.

The x86 Run Inspector launcher leases a persistent checkout keyed by the requesting
worktree, fetches and checks out the exact requested commit, and retains Cargo targets
between runs. Evidence remains private to each run and is removed remotely after harvest.
It keeps fresh private Cargo homes for nested builds. Artifact reuse requires Linux
private mount namespaces, which give each source tree and Cargo home stable compiler
paths without sharing their locks. Uncached direct gates retain the host's normal build.
The userspace cache hashes sources,
local libraries, build/packing scripts, the pinned BusyBox, fonts, Cargo and mke2fs configuration,
profile/compiler-wrapper environment, a private external Rust library snapshot and each workspace toolchain identity. A key is published only after a
clean rebuild produces byte-identical ELFs and disk images. Gate ext2 images populate through libext2fs for deterministic block placement, with explicit geometry, UUID, hash seed and creation time, then normalize
inode timestamps/generations and backup superblock bookkeeping before that comparison.
Every hit checks artifact checksums and copies images rather than sharing writable disks.

`breenix-runs run x86 --fresh` uses a disposable clean checkout and bypasses artifact
reuse for timing comparisons on the same commit, and requires a previously verified
entry for the explicit byte comparison. `[gate-phase]` output records checkout,
userspace build, repacking, clean verification, kernel build, lease waits and boot times;
cache hits report their key and restore time. The shared cache targets 12 GiB total
for lane trees, disposable fresh trees and artifacts; this is a soft budget while entries
are leased. A hard 15 GiB filesystem free-space floor is checked before checkout,
cache misses and kernel builds, including cache hits. This filesystem measurement also
covers canonical sources, legacy clones and run evidence outside the cache. `BREENIX_GATE_CACHE_DIR`, `BREENIX_GATE_CACHE_GB` and `BREENIX_GATE_FREE_GB` on
the execution host override these settings. LRU eviction skips leased entries; an unmet
free-space floor prints GATE: FAIL before building. Post-run eviction is best effort
and cannot change a completed gate verdict. Owner records under owners/ contain the
run id, PID/process birth, host boot id, start time, heartbeat and state; permanent
flocks under locks/ remain the authoritative eviction protection. Normal runs create
no per-run source clone. Fresh trees are removed when their gate ends, including
catchable signals; idle eviction reclaims abandoned fresh trees. Abandoned legacy
clones are reclaimed only after their timestamped run identity is at least an hour
old and a /proc scan of command lines, current directories and open files finds no
owning process, including gates killed before DONE.
Evidence is harvested after the lane lease ends and retained remotely after a disconnect.

`--mode suite --suite files-io,directories,processes` boots once. A sequence file on the
private boot-target disk instructs each suite to exec the next after emitting its own
DONE, keeping PID 1 and giving the next suite a fresh address space. After drawing its panel each sequence suite waits for host acknowledgement. The gate
pauses QEMU at that boundary, saves both serial windows and its screen, and checks
its private raw disk snapshot before allowing the next exec. Each suite gets its own
DONE deadline and unchanged manifest; unstarted suites are NOT-RUN. Exec closes only
FD_CLOEXEC descriptors, while /tmp, PID allocation and the page cache persist, so
case equivalence must be verified against separate boots on the same commit.
Run Inspector files each finished boot with Vigil after harvest, one record per
started suite with its own serial window and start/end times. Duplicate suite IDs are rejected. Single-suite boots keep their
final panel as before. Run cache helper tests with `python3 tests/gate_cache_test.py`.


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
