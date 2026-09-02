# Workload envelopes — Green Program

**R4, 2026-09-01.** Answers a specific gap the assessment named: the program's one
revert (Filesystem x86/aarch64/blended, root cause #728) happened because Filesystem
was declared green against an x86 image that had never run a second concurrent
userspace process. Two days later an unrelated arc (#713, x86 spawn) supplied that
second process and a latent read-park-vs-write-spin race became reachable. No
filesystem code changed between the declaration and the downgrade — the workload the
declaration was proven under simply widened, and nothing had recorded what that
workload was, so nobody could tell the declaration had gone stale until it faulted.

This document records, cell by cell, what workload each **currently standing** green
declaration was actually measured against, so the next widening can be caught by
inspection instead of by a livelock. `tests/green_program_envelope_structure.rs` turns
five of these claims into a mechanical, re-run-on-every-change check (see
[Detector](#detector) below); the rest are recorded here as the honest, human-readable
record and are **not** mechanically enforced — said so at each one.

## How to read this

- Every factual claim below cites the file it came from: an `EVIDENCE-*.md` /
  `CONFIRM-*.md` doc under this directory (what the arc *asserted* it measured), or a
  repo path (what the *current tree* actually does, confirmed by direct reading —
  used where the evidence doc is silent on an axis, or to sharpen a claim the doc
  left implicit).
- Per the claim-discipline this report itself is required to follow: quantitative
  claims are stated as **N of M observed**, not as unqualified universals, and
  "proven" is used only with a named mutation/gate-run cited next to it. Any
  every/all/zero/always/never below is either a direct quote from an evidence doc
  (marked as such) or is immediately followed by the citation that grounds it as a
  literal, checked count — not a rhetorical universal.
- "Standing cells" = the six cells recorded as still HIGH at the moment of the
  2026-09-01 assessment: **TTY–x86 (PR #732), TTY–aarch64 (PR #708/#709),
  TTY–blended (the same PR #732 fix round, coordinator ruling), Tracing–aarch64
  (PR #683), Bus/device infrastructure–aarch64 (PR #723), NIC drivers–aarch64
  (PR #723).** That matches the six the task brief named going in. This list was
  cross-checked against `evidence-declarations.md` §1/§3 during drafting and matched
  exactly — 6 of 6, no fourth arch view among them (no cell has reached HIGH on all
  three views simultaneously in this program's history, per the same table) — but
  that file is an ephemeral workflow artifact under `…/scratchpad/assessment/`, not
  durable in this repo at any commit, so it is **not** cited as this bullet's source
  of truth; the PR numbers above are. A reader on `main` who wants the per-cell
  reasoning behind each HIGH declaration should follow those PRs, not
  `evidence-declarations.md`.

---

## 1. TTY — aarch64

Declared 2026-08-30 (PR #708/#709), amended in the fix round documented in
`tty/EVIDENCE-x86-fix-round-2026-08-31.md`.
Primary evidence: `tty/EVIDENCE-2026-08-30.md`.

**Concurrent userspace processes.** Init's aarch64 launch sequence
(`userspace/programs/src/init.rs::main()`) spawns `/bin/heartbeat` first via
`start_liveness_service()` — a fire-and-forget `spawn()` with no matching `waitpid()`,
so it keeps running for the rest of boot — and then walks a chain of oracle probes
(`run_block_eintr_oracle`, `run_futex_handoff_oracle`, `run_poll_tcp_oracle`,
`run_tty_oracle`, …) each as a `spawn()` immediately followed by a `waitpid()` on that
same child before the next one starts. At the moment the TTY oracle itself runs, the
concurrent userspace process set is therefore **exactly 3**: init (blocked in its own
`waitpid`), `heartbeat` (background daemon), and `/bin/tty_oracle` (the child being
measured) — derived directly from `userspace/programs/src/init.rs` lines 97–136,
165–246, 593–603 (init.rs, current tree). The evidence doc does not itself state this
count; it documents *what the oracle does*, not how many other processes are alive
alongside it — this paragraph is a source-derived supplement, not a quote from
`EVIDENCE-2026-08-30.md`.
No FS-relevant defect follows from this by itself: the oracle's own two-sided arms are
`/dev/pts` (devfs) and `/dev/tty` traffic, never an ext2 read or write (§ below).

**Syscall families exercised** (`tty/EVIDENCE-2026-08-30.md` §2 arm table, 14 arms):
`posix_openpt`/`grantpt`/`ptsname`/`unlockpt`, `open`/`close` on `/dev/pts/N` and
`/dev/tty`, `fcntl(F_GETFL/F_SETFL/F_SETFD)`, `read`/`write`, `ioctl`
(`TIOCSWINSZ`/`TIOCGWINSZ`/`TIOCSPGRP`/`TIOCGPGRP`/`TIOCSCTTY`), termios
get/set/restore, `setsid`, `fork`+`exec`+`waitpid` (arm 14, `cloexec_exec`, aarch64
only). No socket, no ext2-backed file I/O, no signal-delivery syscall
(`kill`/`sigaction`) is exercised — `tty/EVIDENCE-2026-08-30.md` §7 states this
directly: line-discipline signal generation (`^C`→`SIGINT`) is explicitly **not**
covered by this leg (filed as #705) — a real, disclosed gap, not an inferred one.

**CPU count / profile / accelerator.** `-M virt,gic-version=3 -cpu max -m 512 -smp 4`
— confirmed in both `docker/qemu/run-aarch64-tty-oracle-gate.sh:150` and
`docker/qemu/run-aarch64-prod-profile-boot-test.sh:177` (current tree). 4 virtual
CPUs, QEMU's default TCG accelerator (no `accel=` flag set → software emulation, not
HVF/KVM). Kernel build: the shipped **zero-feature production profile** — no
`--features` flag — per `tty/EVIDENCE-2026-08-30.md` §2 ("adding one would make the
gate measure a different kernel than the one that ships").

**Filesystems mounted / writes.** One ext2 filesystem — the QEMU root disk — is
mounted read-write at the block-device level: both gate scripts pass a
`$RUN_DIR/ext2-writable.img` / `$EXT2_WRITABLE` copy with **no `readonly=on` flag**
(`docker/qemu/run-aarch64-tty-oracle-gate.sh:156-157`,
`docker/qemu/run-aarch64-prod-profile-boot-test.sh:183-184`, current tree). No home
filesystem is attached (only one `-device virtio-blk-device` line exists in either
script). The TTY oracle itself issues **zero** ext2 reads or writes at the syscall
level — its I/O is `/dev/pts` and `/dev/tty` only (devfs, not ext2) — but every ELF
that init `spawn()`s, including the oracle itself, is *read* from ext2 by the loader
first (`kernel::fs::ext2::root_fs_read()`, exercised on every spawn, every boot).
`root_fs_write()` (the code path #728 chips) is **never called** by anything this
gate drives — a fact about this workload's syscall surface, not a guarantee about
the kernel: the device permits writes (unlike x86, below), the workload just never
issues one.

**Other fixed axes.** Exactly one ext2-backed disk image, no second block device; one
NIC device present per QEMU's implicit-default-NIC behavior (documented in
`nic-bus/EVIDENCE-2026-08-31.md` §3a for x86 — not independently re-derived for this
aarch64 script here) but never touched by any TTY arm.

**Uncheckable / not measured by this leg.** `ptsname` `ERANGE` behavior and
master-side *blocking* reads (every read the oracle issues is `O_NONBLOCK`) — both
named as open gaps in `tty/EVIDENCE-2026-08-30.md` §7 (#705), not silently absent.

---

## 2. TTY — x86

Declared 2026-08-31 (PR #732), fix round in `tty/EVIDENCE-x86-fix-round-2026-08-31.md`.

<!-- claim-lint:ok: the "after all four" ordering is not prose-only -- it is
     mechanically pinned by tests/green_program_envelope_structure.rs, whose
     x86_tty_oracle_runs_with_no_persistent_background_process walks this exact
     cfg-gated call sequence. -->
**Concurrent userspace processes.** x86's launch sequence in the same `main()` is
`run_spawn_smoke()` (spawn+`waitpid`, sequential) then `run_tty_oracle()` (spawn+
`waitpid`) then `run_exec_smoke()` then, since #745, `run_fork_smoke()` —
`start_bsshd()` and `run_boot_script()` (which spawns `/bin/bsh` and, through it,
seven further processes per #722) come **after** all four, at
`userspace/programs/src/init.rs:131`-`132`; the four `#[cfg(target_arch = "x86_64")]`
launchers themselves are `init.rs:123`-`130`. There is no x86
equivalent of aarch64's `heartbeat` background daemon. So at the moment the TTY
oracle runs on x86, the concurrent userspace process set is **exactly 2**: init
(blocked in `waitpid`) and `/bin/tty_oracle` — no background daemon is alive yet.
This is stated as design intent directly in the source, not just inferred: the
doc-comment on `run_tty_oracle()`'s x86 body says the launcher is "Placed after
`run_spawn_smoke()` and strictly before `start_bsshd()`, so the oracle stays fully
independent of init's boot-script chain (#722) and the production processes that
already run sequentially before it never overlap with bsshd's own ext2 reads
(#728)" (`userspace/programs/src/init.rs:560`-`563`, current tree) — the exact axis
this document exists to make explicit is, for this one launcher, already named in
the source by issue number.

<!-- claim-lint:ok: the "always took an error branch" state is the refusal this
     arc removed, quoted in
     docs/planning/745-x86-fork/serials/anti-vacuity-pre-fix-refused-gate-2026-09-02.txt -->
**#745 addendum.** `run_fork_smoke()` was added to this same sequence, placed
strictly AFTER `run_tty_oracle()` and before `start_bsshd()` (mechanically pinned
by `tests/green_program_envelope_structure.rs`'s `persistent_count_before`, which
only counts launchers before the `run_tty_oracle()` stop marker) — so it does not
change the "exactly 2" concurrent-process claim above, which is about the moment
the TTY oracle itself runs. It DOES widen the workload the *rest* of the boot (from
`run_fork_smoke()` onward) exercises: x86 gains its first production `fork()` +
copy-on-write page-table surface, and `bsh`'s own three `fork()` call sites
(reached later, through `run_boot_script()`'s chain) become newly reachable rather
than dead code that always took an error branch (#745 precheck C13) — disclosed,
not re-derived per-row here since none of the six currently-standing green cells'
own measured workload includes that later portion of boot.

**Syscall families exercised.** **All 14 arms as of #745** (previously 13 of 14 —
arm 14, `cloexec_exec`, was excluded because x86 `fork()` unconditionally refused
in the shipped zero-feature profile, not because `exec()` was `ENOSYS`: that
original exclusion reason (`tty/EVIDENCE-x86-fix-round-2026-08-31.md` §4) had
already gone stale once #721 landed production exec, and this document repeated
the stale reason until #745's own precheck (C14) caught it — #745 is the PR that
actually closed the gap by making x86 `fork()` production-safe, at which point arm
14 was re-admitted on x86 the same round, per `docs/planning/745-x86-fork/`). The
14-arm shared-surface argument for the 13 arms that predate #745 is a census, not
an absent-diff inference: of the 11 files those 13 arms' syscalls dispatch through (`session.rs`, `ioctl.rs`, `tty/ioctl.rs`,
`tty/termios.rs`, `tty/line_discipline.rs`, `tty/mod.rs`, `tty/pty/mod.rs`,
`tty/pty/pair.rs`, `syscall/pty.rs`, `ipc/fd.rs`'s `close_cloexec()`, and
`tty/driver.rs`), **10 of 11 carry zero** `target_arch` occurrences; the 11th,
`tty/driver.rs`, carries **14** — every one of them console byte-out
(`serial_aarch64::raw_serial_char` vs `serial::write_byte`) or arch-specific
diagnostic text, not TTY/PTY protocol semantics, per the source table's own note
(`tty/EVIDENCE-x86-fix-round-2026-08-31.md` §5, which itself corrects an earlier
draft that had claimed this file was zero too). Cited as "10 of 11 zero, the 11th
disclosed," not as a blanket zero across all 11.

**CPU count / profile / accelerator.** `accel=tcg -cpu qemu64 -smp 1 -m 512` —
confirmed in both `docker/qemu/run-x86-tty-oracle-gate.sh:206` and
`docker/qemu/run-x86-prod-profile-boot-test.sh:771` (current tree). **1 virtual CPU**,
TCG. Zero-feature production profile, no `--features` (same discipline as aarch64,
`tty/EVIDENCE-2026-08-30.md` §2).

**Filesystems mounted / writes.** Three disks are attached, **all three
`readonly=on` at the QEMU block-device level**: the UEFI boot image (`id=hd`), a
`placeholder` disk, and the ext2 root disk (`id=ext2disk`) —
`docker/qemu/run-x86-tty-oracle-gate.sh:198-202` and
`docker/qemu/run-x86-prod-profile-boot-test.sh:763-768` (current tree; both verified
directly, not inferred from one). No fourth (`/home`) disk exists in either script,
so `kernel::fs::ext2::init_home_fs()`'s guard (`VirtioBlockWrapper::new(3).is_some()`,
`kernel/src/main.rs:638`) is false and no home filesystem mounts. **Because the ext2
disk is mounted `readonly=on` at the device level, no ext2 write issued on this
workload can ever land on the underlying image — a device-level bound on
*persistence*, not on *lock-path reachability*.** `root_fs_write()`
(`kernel/src/fs/ext2/mod.rs:2183`) is a lock acquisition, not a device write — it
`Ext2WriteGuard`-succeeds the moment it wins `ROOT_EXT2`'s upgradeable-read-then-
upgrade sequence, long before any block I/O is issued, and #728 itself was a
lock-upgrade spin, not a write reaching the disk. So `readonly=on` does **not**
make the #728 code path unreachable on x86 — a second process contending for that
same lock would still spin exactly as it did on aarch64; what the flag guarantees
is that if a write ever *did* land, it could not persist. That is still a real,
disclosed asymmetry from aarch64 (below, ext2 is writable there, so a write that
lands actually sticks) — it is just a narrower one than "hardware-level guarantee
against the defect" claimed. Mechanically checked as a device-flag fact (§
Detector, item 2), not as a claim about lock reachability.

**Other fixed axes.** Same as aarch64: no ext2 read/write is issued by the oracle
itself; every spawned ELF is read from ext2 by the loader on the way in.

**Uncheckable / not measured by this leg.** Same two gaps as aarch64 (`ptsname`
`ERANGE`, blocking master reads), per #705's scope (not re-stated per-arch in the x86
doc, but the oracle body driving both arches is the same, now-14-arm, surface).

---

## 3. TTY — blended

<!-- claim-lint:ok: arm 14's x86 re-admission is 14 of 14 green over 25 boots,
     docs/planning/745-x86-fork/serials/tty-oracle-25boot-soak-2026-09-02.txt -->
Declared in the same fix round, `tty/EVIDENCE-x86-fix-round-2026-08-31.md` §4
(coordinator ruling), **updated #745**: arm 14 (`cloexec_exec`, `fork`+`exec` inside
a PTY session) was aarch64-only supplementary evidence at the time of that ruling,
tracked for x86 re-admission on `#721`'s successor issue (`#745`, since #721 alone
turned out not to be the actual blocker -- fork was). #745 closed that gap and
re-admitted the arm on x86 the same round it fixed production fork, with its own
dedicated multi-boot soak (see `docs/planning/745-x86-fork/`) given the novel
fork-then-immediate-exec interaction the arm exercises. **The blended cell is
therefore now defined at the full 14-arm shared surface**, not 13 with arm 14 held
out. The blended cell's workload envelope is the **intersection** of §1 and §2
above: whichever axis is narrower on either arch view governs the blended claim.
Concretely: 1–4 CPUs depending on which arch view is being exercised (never
simultaneously — no test in this program boots both arches at once and compares
them live), zero-feature production profile on both, all 14 syscall-family arms
common to both as of #745, ext2 mounted on both (writable on aarch64, read-only on
x86 — the blended cell inherits the **weaker persistence bound** (a write that
lands on aarch64 sticks; one that landed on x86 would not), since a blended claim
is only as strong as its weakest view; per §2's correction above, `readonly=on`
bounds whether a write persists, not whether the #728 lock path is reachable, so
the blended cell does *not* inherit any reduced exposure to #728 itself from x86's
flag).

---

## 4. Tracing — aarch64

Declared 2026-08-28 (PR #683). Evidence: `tracing/EVIDENCE-2026-08-28.md`.

**Concurrent userspace processes — partially uncheckable, disclosed.** The harness
(`scripts/test_tracing_via_gdb.sh`) boots a kernel built with `--features boot_tests`
(not the zero-feature production profile TTY uses) — true of the specific run this
cell's evidence cites, but **not enforced by the script itself going forward**: the
script never builds the kernel (confirmed directly, current tree — it only checks
for a pre-built binary at a fixed path) and its own error message, printed when
that binary is missing, recommends the **zero-feature** build command for both
arches (`scripts/test_tracing_via_gdb.sh:100,123`) — not `--features boot_tests`.
So a future run of this exact script against whatever binary happens to already sit
at that path could silently measure a different build profile than the one this
cell's declaration was proven against, with nothing in the script itself to catch
it. It lets the kernel free-run for a settle
window (20s in the cited run), then halts it with a GDB attach and dumps the trace
buffer — it does not stop the kernel at a defined boot stage. The evidence doc's own
citation (`tracing/EVIDENCE-2026-08-28.md` §2, `serials/aarch64-bootgate-markers-…`)
shows only `[BOOT_TESTS:TOTAL:109]` / `[TESTS_COMPLETE:109/109]` — the kthread-based
in-kernel test registry (`kernel::test_framework::executor::run_all_tests()`, whose
own doc comment says it "spawns kthreads to run tests in parallel" —
`kernel/src/test_framework/executor.rs:1-4`, current tree). **`run_all_tests()`
itself never creates a userspace process directly** — confirmed by direct grep:
its own function body contains no call to `create_user_process` or `spawn(`
anywhere (current tree; 0 occurrences). That is a checked fact one level deep, not
a claim about the full 109-test registry: `run_all_tests()` dispatches into 109
individual test bodies living in other modules, and neither this grep nor
`tests/green_program_envelope_structure.rs`'s `boot_tests_registry_stays_kthread_only`
(§ Detector, item 4) walks any of THEIR bodies — a widening inside any one of the
109 registered tests would not be caught by either. What this fact does establish:
the registry's own dispatch loop is kthread-only, whatever any individual test
goes on to do.
What is **not** checked, and is disclosed here rather than assumed: after the
kthread registry completes, the same boot flow continues into
`launch_init_from_elf()` and the ordinary production `init.rs` sequence described in
§1 above (`kernel/src/main_aarch64.rs`, the `#[cfg(feature = "boot_tests")]` block at
the top of `kernel_main` advances to `TestStage::ProcessContext` once the designated
init process exists, then falls through to the same userspace launch path a
zero-feature boot uses). A companion arc's own serial capture shows this
concretely, not just structurally: `[heartbeat] tid=1204 uptime_ms=...` lines
recur throughout the serial log of `docker/qemu/run-aarch64-full-test.sh --rebuild`,
built the same `boot_tests` way
(`docs/planning/green-program/fs/serials/aarch64-full-test-rebuild-20260828.txt`,
lines 90 onward) — `start_liveness_service()`'s heartbeat process actually ran on
this exact profile. (The `#593` red on this same profile is **not** evidence of
this on its own — per `tty/EVIDENCE-2026-08-30.md` §7, it reports that init's
aarch64 boot script spawns no shell, so `run-aarch64-full-test.sh` Phase 2's own
shell check can never *pass* here; a Phase-2 red, alone, says nothing about what
booted before it, since Phase 2 is itself gated on Phase 1 having fully passed
(`if [ -z "$FAIL_REASON" ]`, `run-aarch64-full-test.sh:680`) — which the same
serial capture cited above shows directly: every Phase-1 sub-phase reports PASS
before "Phase 2: Checking services…" begins, lines 19-49 of the cited log.)
So the 20-second settle window in the tracing capture plausibly also included some
or all of the same heartbeat/oracle/bsshd/boot-script userspace-process sequence §1
documents — **but the tracing evidence doc itself never asserts a process count, and
this document does not either.** Treat the true concurrent-userspace-process count
for this cell's measurement as **unknown, bounded below by 0 (kthread registry alone
needs none) and plausibly including the full production init sequence** — an honest
gap, not a claimed number.

**Syscall families exercised.** Not a syscall-driven leg in the TTY sense — it is a
passive read of the kernel's own trace-event ring buffer via a halted-GDB memory
dump. The 28 distinct event types decoded on this run include `TIMER_TICK` and 27
others, all resolving against the kernel's own event-type tables
(`tracing/EVIDENCE-2026-08-28.md` §2); no specific syscall enumeration is claimed or
checked.

**CPU count / profile / accelerator.** `-M virt,gic-version=3 -cpu max -m 512 -smp 4`
(`scripts/test_tracing_via_gdb.sh:187`, current tree) — **4 virtual CPUs**, TCG,
matching the resident-event evidence directly: "4096 (four live CPUs, each a full
1024-entry ring)" (`tracing/EVIDENCE-2026-08-28.md` §2). Build profile:
`--features boot_tests` — **not** the zero-feature production profile TTY and the
other five standing cells were measured against. This is a real, disclosed
divergence in what "the shipping kernel" means across this document's six cells.

**Filesystems mounted / writes.** One ext2 disk, mounted writable (no `readonly=on`)
— `scripts/test_tracing_via_gdb.sh:194` (current tree), same pattern as the other
aarch64 gates. Whether any write actually occurred during the capture is unknown for
the same reason the process count is unknown (§ above): nothing in the evidence doc
asserts it either way.

**Other fixed axes.** x86 tracing was measured the same day
(`tracing/EVIDENCE-2026-08-28.md` §3, `tracing/X86-533-2026-08-28.md`) but **did not**
reach HIGH — it stays MEDIUM on #533/#680/#681 — so it is out of scope for this
document (this report covers standing cells only). The x86 run used `-smp 1`
(`tracing/EVIDENCE-2026-08-28.md` §3: "single-CPU boot: `-smp 1`"), for context only.

---

## 5. Bus/device infrastructure — aarch64

## 6. NIC drivers — aarch64

Both declared 2026-08-31 (PR #723), one arc (arc 5), covered together here because
every leg backing them is shared. Evidence: `nic-bus/EVIDENCE-2026-08-31.md` +
`nic-bus/CONFIRM-2026-08-31.md`.

**What this cell actually measures.** Device-*enumeration* census assertions —
"N VirtIO block, M network devices found," self-counted against each gate script's
own `-device` flags — added to four gates, of which two back the standing aarch64
declaration: `docker/qemu/run-aarch64-full-test.sh` and
`docker/qemu/run-aarch64-service-sequence-gate.sh`
(`nic-bus/EVIDENCE-2026-08-31.md` §2). This is a **boot-time driver-initialization**
check, not a userspace-syscall-driven leg — the assertion fires during PCI/MMIO
enumeration, before any userspace process (even init) exists.

**Concurrent userspace processes.** Same disclosed-unknown shape as Tracing (§4),
for the identical reason: both gates build `--features boot_tests`
(`docker/qemu/run-aarch64-full-test.sh:55`,
`docker/qemu/run-aarch64-service-sequence-gate.sh:116`, current tree), so the
measured boot runs the kthread-based 109-test registry (confirmed kthread-only, same
citation as §4) and then — per the same `main_aarch64.rs` control flow — continues
into the ordinary production userspace-init sequence. The same `[heartbeat]
tid=1204 uptime_ms=...` serial evidence cited in §4
(`docs/planning/green-program/fs/serials/aarch64-full-test-rebuild-20260828.txt`,
`run-aarch64-full-test.sh --rebuild`'s own capture) is the evidence that *this
exact gate script* reaches userspace init in the same boot the device census also
reads — not the script's Phase-2 shell check, which per `tty/EVIDENCE-2026-08-30.md`
§7's `#593` finding has no way to pass on this profile (aarch64's boot script
spawns no shell at all) whether or not Phase 2 is ever reached, so it proves
nothing about the boot either way (see §4's correction for the fuller mechanism).
The service-sequence
gate's 50/50 GREEN run
(`nic-bus/EVIDENCE-2026-08-31.md` §4, table row "aarch64 MMIO total") is a 25-boot,
two-profile battery on the shipped-shape boot sequence, not a bare device-count
microbenchmark — so it plausibly also carries the same heartbeat/oracle/bsshd
concurrency §1 documents, unmeasured and unclaimed by the evidence doc.

**Syscall families exercised.** None directly — the census leg reads driver-emitted
log lines (`pci::enumerate()`'s summary line on x86, `init_virtio_mmio()`'s summary
on aarch64), not syscall return values. No syscall enumeration is claimed for this
cell.

**CPU count / profile / accelerator.** `-M virt,gic-version=3 -cpu "$cpu_profile"
-m 512 -smp 4` (`docker/qemu/run-aarch64-service-sequence-gate.sh:1080`) and
`-M virt,gic-version=3 -cpu max -m 512 -smp 4`
(`docker/qemu/run-aarch64-full-test.sh:145`) — **4 virtual CPUs** both times, TCG,
`--features boot_tests` (current tree, both confirmed directly). The
service-sequence gate's 25-boot batteries ran against **two** CPU profiles
(`max` and `cortex-a72` — "GREEN rate: 25/25 (100.0%) (max) and 25/25 (100.0%)
(cortex-a72)," `nic-bus/EVIDENCE-2026-08-31.md` §4) — the only standing cell in this
document whose evidence explicitly varied the CPU model rather than holding it fixed.

**Filesystems mounted / writes.** Exactly one `-device virtio-blk-device` / ext2 disk
in each script (`docker/qemu/run-aarch64-full-test.sh:151-152`,
`docker/qemu/run-aarch64-service-sequence-gate.sh:1074/1087`, current tree),
writable (no `readonly=on`) in both — same pattern as every other aarch64 gate in
this document. The device census itself never reads or writes ext2; it reads driver
log lines emitted during PCI/MMIO probing, before the filesystem mounts.

**Other fixed axes.** #702 (x86 silent PCI-enumeration hang, ~1/26-52 rate) is the
one open issue capping the *x86* view; it is explicitly stated not to reach aarch64,
because "the aarch64-QEMU arm genuinely never executes `pci.rs` at all"
(`nic-bus/EVIDENCE-2026-08-31.md` §8, `nic-bus/CONFIRM-2026-08-31.md` §9) — the PCI
and MMIO device-discovery code paths are **architecturally disjoint**, not merely
untested on one side. That structural split is why NIC/Bus-aarch64 could reach HIGH
while NIC/Bus-x86 stayed MEDIUM on the same PR.

---

## Cross-cell structural facts

Two axes recur across every cell above and are worth stating once, since they are
exactly the shape of thing that produced the #728 revert:

1. **The ext2 root disk is mounted `readonly=on` on every x86 gate in this document
   and writable on every aarch64 gate in this document** — a real, structural,
   per-arch asymmetry in the filesystem-write envelope, not a coincidence of which
   scripts happened to get checked. Confirmed directly against 7 gate scripts
   (2 x86, 5 aarch64) named throughout §1-6 above; mechanically re-checked by
   `tests/green_program_envelope_structure.rs` (item 2, below) on every future
   change to any of those 7 scripts. Per §2's correction: this is a *persistence*
   bound (a write that lands cannot survive on x86), not a *lock-path reachability*
   bound — `root_fs_write()` is a lock acquisition that succeeds independently of
   the device flag, so #728's own lock-upgrade spin is reachable on either arch.
2. **Two of the six standing cells (Tracing-aarch64, Bus/NIC-aarch64) were measured
   on the `boot_tests` feature profile, not the zero-feature production profile the
   other four (TTY, all three views) were measured on** — and the `boot_tests`
   profile's own boot flow continues into the same userspace-process sequence TTY's
   envelope documents, on top of a 109-test kthread registry TTY's profile never
   runs at all. This document does not know, and does not claim to know, exactly how
   many userspace processes were alive at the moment either cell's measurement was
   taken — see §4/§5-6's "uncheckable" notes. **This axis is enforced on two of
   the three scripts, and unenforced only on the third.** `run-aarch64-full-test.sh`
   and `run-aarch64-service-sequence-gate.sh` each define and call
   `require_boot_tests_kernel()` *unconditionally* — outside, and after, their
   `if $REBUILD` block (which closes at line 61 and line 122 respectively; the guard
   itself is called at line 111 and line 185) — so every run, `--rebuild` or not, is
   checked against a census of marker literals that only a `--features boot_tests`
   kernel emits, and the script `exit 1`s on any miss. The two censuses are not
   identical: `run-aarch64-full-test.sh` checks 7 markers (`SCHED_STRAND_ORACLE`,
   `STRAND_INJECT_ORACLE`, `CENSUS_WIDEN_ORACLE`, `FUTEX_HANDOFF_ORACLE`,
   `CTX596_ORACLE`, `TOMBSTONE_JOIN_ORACLE`, `BOOT_TESTS`);
   `run-aarch64-service-sequence-gate.sh` checks 6 (the same set minus
   `TOMBSTONE_JOIN_ORACLE`). Both name themselves, in a code comment, the twin of
   the #528 guard, and warn about the same landmine: any `cargo test` run in the
   same session silently rebuilds the kernel without `boot_tests` and hardlinks it
   into the shared output path in a fraction of a second, swapping the binary the
   next gate boots. `scripts/test_tracing_via_gdb.sh` is the one script this axis
   really is unenforced on: it never builds at all (§4 — its two `cargo build`
   strings live only inside `--help` text) and carries no `boot_tests` guard of any
   kind. What the two guarded scripts' census does *not* check is source freshness:
   it verifies the binary belongs to the `boot_tests` *profile* (the right feature
   set was compiled in), not that it was built from the exact commit the cited
   evidence doc measured — a stale `boot_tests` binary from an earlier commit passes
   the same census. So the profile claims above are still sourced to the specific
   historical runs the cited evidence docs measured; the census is a standing
   guarantee for profile membership, not for source-commit freshness.

---

## Detector

`tests/green_program_envelope_structure.rs` turns five of the claims above into a
host-side structural test (`cargo test --test green_program_envelope_structure`),
run the same way every other `*_structure.rs` ratchet in `tests/` is run — no kernel
build or QEMU boot required, since every check parses source/script text directly.
It is **not** a CI gate (this repo has no GitHub Actions and no git hooks are in
scope per the task); it is a test the sweep process — or any future arc touching
`init.rs`, the seven cited gate scripts, `executor.rs`, `kernel/src/syscall/handler.rs`,
or `kernel/src/arch_impl/aarch64/syscall_entry.rs` — is expected to run before
declaring or re-declaring a cell, the same way `tty_oracle_structure.rs` is already
run before every TTY declaration. It covers only these six *currently standing*
cells; if Filesystem or any other cell re-declares, this document and this test file
do not automatically grow a section or a check for it — extending both to a newly
green cell is a manual follow-up, out of this task's scope.

**What it checks.** Every item's *extraction* reads the current fact out of the
source it governs — never string-matching against this document's own prose. Item
1 and item 3's *expectations*, however, are hand-typed literals pinned to this
document's own numbers (a ratchet, not a derivation from anything else in the
tree) — stated plainly here rather than as "never a hand-typed value," which items
1 and 3 make false if taken literally:

1. **TTY concurrency invariant.** Parses `userspace/programs/src/init.rs::main()`'s
   call sequence and classifies each helper it calls, before the arch-appropriate
   `run_tty_oracle()` call site, as *persistent* (contains `spawn(`/`spawnv(` with no
   matching `waitpid(` in the same function body — i.e. still running when the next
   call starts) or *reaped* (both, sequential). Asserts the aarch64 persistent count
   is exactly 1 (`heartbeat`) and the x86 persistent count is exactly 0, matching §1
   and §2 above. A future PR that adds a second background daemon before either
   arch's `run_tty_oracle()` call, as **text in `init.rs`**, reddens this test by
   name instead of silently changing the concurrency envelope TTY was proven under.
   This is *not* the shape #713 actually took (item 5, below, is) — #713's own
   `init.rs` call sites for `start_bsshd()`/`run_boot_script()` already existed as
   text before the merge; only their runtime effect changed via a kernel dispatch-
   table edit this item cannot see.
2. **ext2 read-only/writable split.** Parses the ext2 `-drive` (or `drive_opts=`)
   declaration out of all 7 gate scripts named in §1-6 and asserts x86 scripts carry
   `readonly=on` and aarch64 scripts do not. Flags the moment any of the two x86
   scripts drops `readonly=on` (a real widening of the persistence bound described
   in §2) or either direction drifts from what this document claims.
3. **`-smp` census.** Parses the `-smp N` value adjoining each gate script's own
   arch-marker line (`-M virt,gic-version=3` for aarch64, `-machine pc,accel=tcg` for
   x86) for the same 7 scripts and asserts aarch64 is 4 and x86 is 1, matching every
   CPU-count claim above.
4. **kthread-only boot_tests registry.** Parses
   `kernel/src/test_framework/executor.rs::run_all_tests()`'s body and asserts it
   calls neither `create_user_process` nor `spawn(` — the fact §4/§5-6 lean on to
   say the 109-test registry itself never becomes a second concurrent userspace
   process. This checks only `run_all_tests()`'s OWN body, one level deep — it says
   nothing about the 109 individual test bodies it dispatches into, which live in
   other modules and are not walked by this check; a widening inside any one of
   those 109 tests would not be caught here.
5. **Syscall-dispatch census.** Parses `kernel/src/syscall/handler.rs`'s live x86
   dispatch table (`rust_syscall_handler`) and `kernel/src/arch_impl/aarch64/
   syscall_entry.rs`'s live aarch64 dispatch tables (`rust_syscall_handler_aarch64`
   plus `dispatch_syscall_enum`) for the `SyscallNumber` variant named in each arm
   (all 126 variants the enum declares, on both arches — a literal count from the
   standalone harness run described below, not assumed), classifies each arm as a
   real dispatch or a hardcoded ENOSYS stub, and asserts the current ENOSYS-stub
   set is exactly `{GetTime}` on x86 and exactly `{ArchPrctl}` on aarch64. **This is
   the axis #713 actually widened**: Spawn moved out of x86's
   stub set in commit a60b8855 with zero change to `init.rs` text, which is exactly
   why items 1-4 above missed it. Replaying the real `handler.rs` bytes from
   immediately before and immediately after PR #730 (#713's fix) through this exact
   census logic (verified standalone, not committed as a test here — see item 5's
   own code comment) shows the stub set changing across precisely that commit, with
   all 125 non-Spawn arms byte-identical between the two.

Every item above has at least one **WIDENING** mutation proof (a change modeled on
a real capability change is asserted to flip the check from green to red) and at
least one **CONTROL** mutation proof (a change the check does not own is asserted
to leave it unaffected), each applied to an in-memory copy of the real file content
(`String::replace`, never an on-disk edit, matching `tty_oracle_structure.rs`'s
established idiom) and fed through the exact same extraction function the live
check uses. Items 1, 2, 3, and 5 each prove both their x86 and aarch64 arm
specifically, not just one arch standing in for both — closing a gap a review of
this suite's first draft found (two arms, including the one that models #713's own
*shape* most directly, had no mutation proof at all). All mutation-proof tests pass
against the current tree; see `tests/green_program_envelope_structure.rs`'s own
test functions for the exact before/after pairs — nothing here restates them as a
second copy that could drift from the code.

**What the detector does not, and cannot, cover** — stated plainly rather than
implied covered:

- It has no opinion on **whether** a widening it detects is actually unsafe — it
  flags "the envelope this document described no longer matches the tree," which is
  a prompt to re-derive and re-prove the affected cell, not an automatic proof that
  the cell is now broken (the FS incident needed a livelock's worth of reasoning
  beyond "a second process now exists").
- It cannot check any axis that is not textually present in a script or source file
  it parses — in particular, it says nothing about the **uncheckable** axes
  disclosed in §4/§5-6 (real concurrent-process counts once a `boot_tests`-profile
  boot continues past the kthread registry), the **syscall families** axis for any
  cell (free-text prose above, not machine-checked anywhere), or **read/write
  behavior at the syscall level** (it checks the QEMU block-device flag, which is a
  necessary condition for x86 writes to be impossible and a sufficient condition for
  nothing to have observably changed on that axis — it is not a claim about what any
  given syscall sequence does).
- It does not run against `main` on a schedule — nothing in this repository does
  (no CI, no git hooks in scope here); it is discovered and run manually, the same
  way every other `tests/*_structure.rs` ratchet already is.

**Known parsing limits, disclosed rather than fixed (minor, found in R4 review):**

- `ext2_drive_is_readonly` returns on the *first* line matching `id=ext2disk` or
  `id=ext2,` and is keyed to those two exact spellings. A script that gains a
  second ext2 disk (`run-ext2-lock-race-gate.sh` already has one,
  `id=ext2root`/`id=ext2home`, though it is not one of the 7 watched scripts) would
  only have its first match read; a third spelling would panic loudly rather than
  silently misreading. Benign today: all 7 watched scripts have exactly one match,
  verified directly.
- `classify_launcher`'s cut is "does the child outlive the launcher call," but the
  axis #728 actually needed is "is a second process alive at all, concurrently
  enough to contend for ext2." A *reaped* launcher inserted immediately before the
  oracle still makes the kernel's ELF loader read from ext2 with a second process
  briefly live, and item 1 would score it 0 (no change). The persistent/reaped cut
  is defensible for the TTY cell specifically (its own declared envelope really is
  about the background daemon that outlives everything); as the general "widening"
  primitive the Detector section above sells it as, it is narrower than the real
  axis.
- `main_call_sequence` only recognises bare `ident(...);` statement lines that end
  literally `");"`. A launcher invoked as `let _ = start_daemon();`, or with its
  call split across lines, is silently skipped — not a panic, a silent zero that
  would undercount the persistent-launcher census.
- §1's "exactly 3" concurrent-process derivation does not mention
  `run_init_group_refusal_probe("early")`, which runs before the TTY oracle on
  aarch64 and issues a raw `clone` via inline asm rather than `spawn()` — expected
  to be refused (`-22`), so it does not add a fourth process and the count of 3
  stands, but the omission from the derivation's own prose is exactly the kind of
  unstated step this document is elsewhere careful to name.
- Item 4's `body.contains("spawn(")` also matches `respawn(` / `kthread_spawn(` if
  either ever appeared in `run_all_tests()`'s body. This over-matching, on its own,
  is a false-positive risk only (the check would redden on a text change that isn't
  actually a widening) — over-matching a substring cannot itself cause a miss. It
  says nothing about item 4's separately-disclosed false-negative gap above (any
  userspace-creating API not literally named `create_user_process` or `spawn(`, or
  any widening inside one of the 109 individual test bodies, is missed regardless
  of this substring's own behavior).

---

## Claim-discipline self-check

Per the task's own instruction, this document was grepped for
`every|all|zero|always|never` before finishing this R4 fix round: **93 hits**
(`grep -owE 'every|all|zero|always|never' docs/planning/green-program/WORKLOAD-ENVELOPES.md
| wc -l`, regenerated after every edit in this pass, including this sentence, not
left stale from an earlier draft — up from the original draft's 67 because this
round added a fifth detector item, a corrected filesystem-persistence argument, and
a disclosed-limits section, each of which legitimately needs some of these words).
Each was read in context and falls into one of the patterns below — a direct
quote, a literal cited count, or a scoped/disclosed statement rather than an
unbacked universal; representative examples of each pattern are listed, not all 93
individually:

- "**never** overlap with bsshd's own ext2 reads" (§2) — a direct quote from
  `userspace/programs/src/init.rs`'s own doc comment, attributed as a quote, not
  this document's claim.
- "**zero** ext2 reads or writes" (§1), "10 of 11 files carry **zero**
  `target_arch` occurrences [the 11th disclosed at 14]" (§2, a literal re-counted
  table), "run_all_tests()… 0 occurrences" (§4) — each is a direct grep/count
  result stated next to its method, not an unqualified universal.
- "no test in this program boots both arches at once" (§3) — scoped to "this
  program," not a claim about what's possible.
- "This document does not know… **never** asserts a process count" (§4) — a
  disclosure of a limit, not a universal claim about the world.
- "so #728's own lock-upgrade spin is reachable on either arch" / "`root_fs_write()`
  is a lock acquisition that succeeds independently of the device flag" (§2, cross-
  cell fact 1) — the corrected persistence-vs-reachability argument, each half
  grounded in the cited function's own source (`kernel/src/fs/ext2/mod.rs:2183`),
  not a restated "hardware-level guarantee."
- "the ENOSYS-stub set is exactly `{GetTime}` on x86 and exactly `{ArchPrctl}` on
  aarch64" (Detector item 5) — a literal set, re-derived by the census described in
  the same sentence and re-checked by two live `#[test]`s, not an unqualified
  "nothing is ever ENOSYS."
- "**zero**-feature production profile" (throughout) — a proper noun (the profile's
  own name in this codebase, `cargo build` with no `--features` flag), not a
  quantifier.
- Detector section: "asserts… neither… nor," "leave the check unaffected," "does not
  run against `main` on a schedule — nothing in this repository does" — each is
  either a specific assertion inside a named test function (checkable by reading
  that function) or a scoped negative about this repo's own CI/hook surface
  (verified: no `.github/workflows`, no git hooks referenced anywhere in this task).
- Known-parsing-limits section: "over-matching a substring cannot itself cause a
  miss" (item 4's `spawn(` substring match) — a narrow, scoped claim about that one
  substring's own behavior, immediately followed by a cross-reference to item 4's
  separately-disclosed, real false-negative gap rather than a claim the check is
  flawless.
