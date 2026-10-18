# The boot path

As of 2026-10-18 (S23), x86-64 requires evidence from x86-metal using the metal boot loop.
The main path measures ARM64 QEMU, Parallels and VMware. Earlier x86 QEMU gate
and landing requirements below are superseded by [the execution policy](x86-execution-policy.md);
legacy QEMU commands require the operator's `BREENIX_ALLOW_X86_QEMU=1`.

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
stage list on the x86-metal path; it is not measured on the main path.

## Boot modes

### ARM64 QEMU acceleration

On an Apple Silicon Mac, ARM64 QEMU defaults to `-accel hvf -cpu host` with
`virt,gic-version=3`, including `scripts/boot-interactive.sh` and `run.sh`.
Use `--accel tcg` to select software emulation explicitly. All native ARM64
launchers and xtask accept `BREENIX_ARM64_QEMU_ACCEL=hvf|tcg` through
`scripts/qemu-arm64.py`. Named CPU profiles default to TCG, preserving their
CPU and GIC coverage; explicit HVF refuses CPUs other than `max` or `host`.
Debugger and watchpoint-oracle helpers explicitly select TCG. Each launch
prints `BREENIX_QEMU_CONFIG` with its effective accelerator and CPU for host records. An explicit HVF request on another host
fails instead of falling back to TCG. The Linux Docker launchers select TCG:
Apple's Hypervisor framework is available only to native macOS processes.

The shared accelerator wrapper runs `scripts/qemu-ui-launch.sh`'s launcher
immediately before QEMU. It clears inherited Darwin background mode, nice
priority and latency/throughput tiers before requesting the existing
user-interactive QoS class (thread priority 46). A lane's `taskpolicy -b`
therefore applies to its builds, not its boot. The canonical gate also runs
its Mac build subprocesses under `taskpolicy -b` by default.

A spawn QoS clamp survives fork and exec. When the launcher cannot attain
the boot priority, `qemu-unclamp.py` uses a temporary Interactive launchd job
and application spawn policy to start an unclamped task. It transfers the
original console descriptors and environment over a private Unix socket,
returns the payload's exit status, and requests QEMU termination and job removal when
the caller exits or dies. After supervisor SIGKILL, QEMU termination can take
up to five seconds; job removal in that path is not asserted by the host test.
Ordinary backgrounded launches still exec in place.
Launcher build or priority-restoration failures stop the boot.
`BREENIX_QEMU_UI=0` explicitly bypasses this policy for diagnostics; it does
not change the accelerator. The Mac boot slot serializes supervised boots;
the SIGKILL cleanup delay above can outlive the supervisor's slot lease.
Boots start without waiting for host load to fall. Keep the VM window open on a Mac
(`--display`); apply background policy to builds rather than an entire landing-gate command.

Parallels VMs get the same class: `run.sh` creates each one with the system flag
`vm.vcpuprio=6`, so `prl_vm_app` starts its vCPU threads at user-interactive QoS
(priority 46) instead of the default class (priority 31) and declines later requests
to change it. `run.sh` refuses to start a VM whose `config.pvs` does not carry the flag.

QEMU-HVF remains a separate target from Parallels and VMware: its VirtIO
devices and emulated GICv3 remain QEMU's. QEMU 11.0.3 supports that GIC with
HVF. Its MMIO emulator requires ESR.ISV=1. ARM64 VirtIO device registers,
GIC registers and fw_cfg use shared register-indirect assembly helpers;
DMA-ring RAM retains Rust volatile accesses. Compiler-selected address-writeback
instructions cannot be emulated by this QEMU version.

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
  the VM, and exit nonzero for a missing completion or `failed>0`. The x86-64
  production kernel runs `/sbin/probe` for the same boot-target file (the x86-metal
  storage boots use it; `docs/metal/README.md`, "The disks").
- `shell`: the production kernel runs `/sbin/init shell`, a bsh prompt on the serial
  console you can type into.
- `desktop`: the production kernel runs `/sbin/init desktop`, the window manager and a
  terminal in the VM window (`--no-display` keeps it headless). The window manager
  composites through VirGL, which only the Parallels and VMware GPUs provide: under the
  QEMU launchers it prints `GPU compositing required` and exits after init reports the
  services started.
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
  (one line, `suite ID`, or `suite ID case CATEGORY/CASE` for `--case CATEGORY/CASE`) onto a copy of the ext2 disk. The kernel takes the fw_cfg mode
  first (x86-64 QEMU reads fw_cfg too, for `default` and `suite`), then that file, else
  `default`. Suite mode exercises the kernel milestones and the first userspace process: the
  suite's `START` and `DONE` lines show PID 1 running in user mode and making syscalls.
  `--case CATEGORY/CASE` selects one case on ARM64 QEMU, Parallels and VMware. QEMU passes
  it through `opt/breenix/case`; the kernel forwards `--case CATEGORY/CASE` in the
  suite's argv. START and DONE carry `only=CATEGORY/CASE`, which marks the run as not
  full-suite evidence. Unknown IDs fail with the valid IDs (see `docs/suites/README.md`).
  ARM64 QEMU waits two seconds after a complete `DONE` line for rendering, then
  holds the scored panel for `BREENIX_SUITE_HOLD` seconds (default 5), continuing
  to watch for fatal output. It stops the VM, validates the records against the
  manifest and releases the shared boot slot. A missing DONE or fatal kernel
  output fails the boot; its idle exit and `--gate-timeout` DONE deadline also
  stop unfinished suites. Parallels and VMware stop the VM, record the result and
  release the boot slot after scoring DONE. VMware deletes only the stopped bundle exclusively created by the current
  runner; unowned bundles are never swept. `--suite --test` on Parallels
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

The required testing programs use Breenix's Rust runtime; legacy musl C
programs are outside this gate, pending replacement by Breenix's own C library.
The DNS tests fork a UDP resolver bound to guest loopback; they verify exact
local A answers and NXDOMAIN responses without external DNS or Google access.
This also exercises DNS on VMware while its host NAT repair (#1202) is pending;
it does not establish that VMware can reach an external network.

## Shared host slots

The x86 gate details below describe the retired QEMU path, available only with an
operator override. They are not a landing checklist. Use the x86-metal boot loop
for x86-64 evidence. ARM64 boots use the Mac boot slot; builds use four Cargo jobs
and background priority, and boots run at normal priority with their windows open.
Do not wait for a quiet host.

Use `docker/qemu/run-x86-gate.sh` for queued x86 builds and boots: the helper
provides `x86-build` leases and one `x86-boot` lease. Run Inspector installs its current
helper outside the tested checkout, including when testing an older revision.
Historical gates hold both leases for their whole run. Launchers sourcing
`docker/qemu/lib/qemu-host-lock.sh` also enroll in the host queue; on Linux they
hold a build lease until boot admission. Mac builds remain unrestricted.

The gate VM boots; it does not build. When the VM's private host file
(`~/.config/breenix-gate/build-host.env`) names the x86 build container and the commit
carries `scripts/x86-gate-build.py`, `scripts/gate-tree.py` runs the gate's build phase
(`BREENIX_GATE_PHASE=build`) in that container over ssh, then the boot phase in the VM. Missing
configuration or a commit without that builder fails before building; there is no VM build
fallback. A direct `docker/qemu/run-x86-gate.sh N MODE` in a clean private clone of the VM's
canonical repository hands the whole gate to `scripts/gate-tree.py` for the clone's commit,
in one cached tree shared by direct calls, with a unique run directory beneath
`BREENIX_GATE_TMP`. Before handoff, the target guard checks the private clone's local effort
refs against the project's remote heads and forwards the checked commit stamp. A gate-cache
tree asking for both phases comes from an older launcher and is refused. Each tree's `origin`
is the canonical repository's own remote, so the boot's effort target guard reads the
project's branches rather than the canonical cache's stale local ones. The container keeps its lane checkout leased until its outputs finish streaming. The
container's cpuset is the boot host's shared CPUs, outside the cores reserved for the gate
VM, at the lowest CPU priority and with its own memory limit. It checks the commit out at
the same path as the VM's tree, because the launcher has absolute paths compiled in, and
builds there exactly as the VM did: `scripts/gate-artifacts.py` publishes userspace only
after its candidate and clean-verification builds compare byte-identical, in the
container's own cache, which holds no metal entries. The launcher, the UEFI image it
names, the disks and the ELFs come back to the VM's tree (holes kept), with the build's
logs in the run's `gate-tmp/build-logs-container/` and its result, including the CPUs it
ran on, in `gate-tmp/x86-build.json`. The kernel ELF stays in the container's tree. The
container's `x86-build` leases are its own: their count is its CPUs over each build's 4
Cargo jobs, bounded by its memory limit over 4 GiB a build (`build_slots` in
`scripts/host-slots.py`), and no boot waits for them or freezes them. Two metal builds
compiling on the shared cores through a whole time-suite boot left it at 92/92 (run
`20261017T205020Z-x86_64-gate-4f09`). A commit without the build phase still builds in
the VM, as below.

A guest's vCPUs are host threads, and a guest charges the time a busy host leaves them
unscheduled as running, so timing cases fail on a loaded host. A build inside the gate VM
shares the boot's vCPUs, so an x86 boot first holds every build lease in the VM (after
builds queued before it, ahead of those queued after it), then its boot lease, and keeps
both until it ends: no build runs in the gate VM during a boot. A boot request queues for
both in one step, and a session holding a build lease (a historical gate) gives it up while
it waits for all of them, so two boots never wait on each other. claim-lint:ok: the lease order is pinned in
`tests/host_slots_test.py` (`test_boot_holds_every_build_lease_until_it_ends`, `test_two_sessions_holding_build_leases_both_take_the_boot`).

Every launcher calls `host_slot_quiet`, with the manifests of the suites it runs, just
before it starts its VM: `scripts/boot-interactive.sh`, each VM path of `run.sh`, and
`docker/qemu/run-x86-gate.sh` before each boot, which also covers Run Inspector. No boot
waits for a quiet host by default: the wait held the boot lease behind other work, for up to
45 minutes on the Mac and 35 minutes in the x86 gate VM, while every other lane queued, and
on the Mac it measured no smaller guest stalls (#1376, #1383). Boots record their load (and,
in the x86 gate VM, the vCPU gap and steal) so a timing failure can be compared with main
under the same load. Nothing limits other work on either host. For one invocation,
`BREENIX_QUIET=1` or `0` forces or skips the wait, and `BREENIX_QUIET_MAX_LOAD`,
`BREENIX_QUIET_MAX_GAP_MS` and `BREENIX_QUIET_MAX_WAIT` (seconds) replace the thresholds
and the cap.

Each boot's record in its `*.host-slots.jsonl` sidecar carries the load at enqueue, at
acquisition, at the VM's start, at the verdict (the serial's first `SUITE <id> DONE`,
`PROBE DONE`, `USERSPACE TEST REPORT DONE` or `RUN PATH DONE` line, updated by later
ones) and at release, and whether a quiet host was required and found. On Linux it also
carries the vCPU gap at the start and the steal time from the start to the verdict: the
physical host's contention as the gate VM saw it.

Each x86 gate boot also keeps `host-stall.jsonl` beside its serials, written by
`scripts/x86-gate-host-stall-sampler.py` while the boot runs, sampled at a 20 ms period: the guest's TSC
(this host's TSC plus KVM's offset for the guest), each gate VM CPU's steal, and each of the
guest's vCPU threads' run delay, run time, host CPU and state. The guest cannot write it.
`scripts/score-sigkill-teardown.py` reads it to decide whether a SIGKILL kick window the
guest calls starved was starved by the host.

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
Run Inspector streams both beast serial logs into its local run directory over a separate
owned SSH connection. It registers the gate with Vigil at launch, before building or receiving
serial bytes, with the launcher's PID and links to the local files. The gate runs in a detached remote session with an exclusive per-run launch claim,
so losing either SSH connection cannot cancel it or start a second gate. The Mac
reconnects with bounded consecutive retries, resumes build/boot output and serials at
their byte offsets, and deduplicates serial replay; `reconnects.jsonl` records each attempt. The single `beast` SSH alias follows the Mac's SSH configuration.
Local signals, owner death and exhausted gate retries request cancellation with a durable
owner marker, then harvest evidence after quiescence. Serial failures only degrade the live
view. If cancellation cannot reach beast, the launcher names the remote evidence and owner,
warns about the lane lock and provides `breenix-runs recover x86 <run-id>` to stop, harvest
and clean up later. Completed launch tombstones are retained for seven days and pruned on
subsequent launches; running owners are never pruned.
After the remote owner records its final result it stops the stream and pulls the remote evidence before finishing Vigil. Interrupted gates,
failed builds and failed evidence pulls finish with nonzero status; captured remote evidence
is harvested after the supervisor has stopped. Runs with no boot directories still receive a
finished record. Multi-suite sequences keep an overall gate record and file each started
suite separately with its own serial window, times and result. Registration subprocesses are
bounded and run outside the serial frame lock. The stream reader uses nonblocking writes so
EOF and its heartbeat deadline remain effective when output stalls.
Duplicate suite IDs are rejected. Single-suite boots keep their
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

- `--display` opens the screen in a window. On a Mac the window belongs to a QEMU app bundle
  (`scripts/qemu-1x-app.sh`), which turns App Nap off for itself: a napped QEMU fires the
  guest's timer ticks tens of milliseconds late, and the time suite's limits fail (#1292).
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

## Stub milestones

A milestone with `"stub": true` is a definition under construction, not a completion claim: its stage
lists are proposals that no kernel prints yet, so no boot passes it. The lane that produces its markers
lands the exact strings with its code and the stage catalog, and removes the flag. `boot-logo` is one: the
console effort's splash (`docs/console/contract.md`, section 7), finalized by its splash lane. Because a
mode list replaces the architecture list, its `shell` and `default` lists repeat its four kernel stages
before the hand-over stage, and each names the one hand-over reason that mode must reach.

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

`boot-interactive.sh`, native `run.sh` gates and x86 gate runners stop
an unfinished boot after
120 seconds without classified test progress, independently of the hard deadline
or raw-serial idle exit. Shell and desktop sessions keep their interactive idle
policy (`--idle-exit 0` disables idle shutdown). GDB-halted sessions are excluded
from historical supervision. Queue and build time do not start the watchdog. xtask keeps its existing
launch policy: #1450 tracks its BTRT caller accepting partial startup KTAP after
a failed boot. Its watchdog integration requires that caller to propagate
failure and complete guest-memory extraction before stopping a successful VM.
Progress on a mixed UART requires a userspace record, a tag found in the
userspace program sources, or a first-seen
boot-stage marker. Unlabelled lines have no trustworthy producer even on COM1,
which also carries kernel debug output.
Loader announcements and diagnostic records do not extend the deadline.
A stall exits nonzero and retains serial plus a neighboring `*.stall.json`
record containing the last progress line, last userspace progress line, their
sources and timestamps, and the UTC stop time. x86 gate watchers use the userspace
serial as the sidecar's stable location. Mac serial defaults include run identity.
The x86 gate relays Unix socket serial channels into files as bytes arrive.
Run Inspector retains a typed `stalled` verdict and installs a compressed helper
bundle outside the tested checkout. Historical source keeps its original policy
unless the caller sets `BREENIX_WATCH_HISTORICAL=1`; that option enrolls owned
file-serial QEMU processes at launch. Generic qemu-host-lock launchers are otherwise
unchanged. A completed panel is outside the stall deadline; a suite sequence
resumes monitoring when the next suite prints START.

The suite runner's default case deadline is 10 seconds. The host source check
looks for builder calls, public-field struct literals and assignments throughout
`userspace/programs/src`, including helper modules. The one named allowance,
`time/clocks/rate-counter`, has 25 seconds for its bounded RTC measurement;
every other case retains the default. The host check validates every runner allowance
against the manifest and the unchanged 120-second stall limit. The progress monitor
does not extend that limit for named allowances.
Silent operations that exceed
the watchdog window require meaningful userspace progress before the deadline;
periodic kernel output cannot provide that allowance.
