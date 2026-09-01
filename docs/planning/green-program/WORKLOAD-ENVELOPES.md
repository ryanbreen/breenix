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
four of these claims into a mechanical, re-run-on-every-change check (see
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
- "Standing cells" = the six cells `evidence-declarations.md` §3 recorded as still
  HIGH at the moment of the 2026-09-01 assessment (verified against that document,
  not assumed from the task brief): **TTY–x86, TTY–aarch64, TTY–blended,
  Tracing–aarch64, Bus/device infrastructure–aarch64, NIC drivers–aarch64.** That
  matches the six the task brief named going in; this document independently
  re-derived the list from `evidence-declarations.md` §1/§3 rather than trusting the
  brief, and it matches exactly — 6 of 6, no fourth arch view among them (no cell has
  reached HIGH on all three views simultaneously in this program's history, per the
  same table).

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

**Concurrent userspace processes.** x86's launch sequence in the same `main()` is
`run_spawn_smoke()` (spawn+`waitpid`, sequential) then `run_tty_oracle()` (spawn+
`waitpid`) then `run_exec_smoke()` — `start_bsshd()` and `run_boot_script()` (which
spawns `/bin/bsh` and, through it, seven further processes per #722) come **after**
all three, per `userspace/programs/src/init.rs` lines 123-130. There is no x86
equivalent of aarch64's `heartbeat` background daemon. So at the moment the TTY
oracle runs on x86, the concurrent userspace process set is **exactly 2**: init
(blocked in `waitpid`) and `/bin/tty_oracle` — no background daemon is alive yet.
This is stated as design intent directly in the source, not just inferred: the
doc-comment on `run_tty_oracle()`'s x86 body says the launcher is "Placed after
`run_spawn_smoke()` and strictly before `start_bsshd()`, so the oracle stays fully
independent of init's boot-script chain (#722) and the production processes that
already run sequentially before it never overlap with bsshd's own ext2 reads
(#728)" (`userspace/programs/src/init.rs:547-550`, current tree) — the exact axis
this document exists to make explicit is, for this one launcher, already named in
the source by issue number.

**Syscall families exercised.** Same 13 of the 14 arms as aarch64 (§1) — arm 14,
`cloexec_exec`, is excluded because `exec()` is `ENOSYS` in the shipped x86
zero-feature profile (`tty/EVIDENCE-x86-fix-round-2026-08-31.md` §4). The 13-arm
shared-surface argument is a census, not an absent-diff inference: every file the 13
arms' syscalls dispatch through (`session.rs`, `ioctl.rs`, `tty/ioctl.rs`,
`tty/termios.rs`, `tty/line_discipline.rs`, `tty/mod.rs`, `tty/pty/mod.rs`,
`tty/pty/pair.rs`, `syscall/pty.rs`, `ipc/fd.rs`'s `close_cloexec()`) carries **zero**
`target_arch` occurrences (`tty/EVIDENCE-x86-fix-round-2026-08-31.md` §5, a table of
11 files each re-counted directly against the tree) — a literal, cited zero, not an
unqualified "shared code" claim.

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
disk is mounted `readonly=on` at the device level, `root_fs_write()` cannot succeed
on this workload even if something called it — not merely "nothing calls it," as on
aarch64, but a hardware-level guarantee.** This is the sharpest asymmetry between the
two arch views' filesystem envelopes and is mechanically checked (§ Detector, item 2).

**Other fixed axes.** Same as aarch64: no ext2 read/write is issued by the oracle
itself; every spawned ELF is read from ext2 by the loader on the way in.

**Uncheckable / not measured by this leg.** Same two gaps as aarch64 (`ptsname`
`ERANGE`, blocking master reads), per #705's scope (not re-stated per-arch in the x86
doc, but the oracle body driving both arches is the same 13-arm surface).

---

## 3. TTY — blended

Declared in the same fix round, `tty/EVIDENCE-x86-fix-round-2026-08-31.md` §4
(coordinator ruling). **Defined, explicitly, at the 13-arm shared surface** — not a
14-arm claim with a footnote. The blended cell's workload envelope is therefore the
**intersection** of §1 and §2 above: whichever axis is narrower on either arch view
governs the blended claim. Concretely: 1–4 CPUs depending on which arch view is being
exercised (never simultaneously — no test in this program boots both arches at once
and compares them live), zero-feature production profile on both, 13 syscall-family
arms common to both, ext2 mounted on both (writable on aarch64, read-only on x86 —
the blended cell inherits the **weaker** (write-permitting) guarantee, since a
blended claim is only as strong as its weakest view). Arm 14 (`fork`+`exec` inside a
PTY session) is aarch64-only supplementary evidence, tracked for re-admission on
`#721`, per the same ruling — explicitly **not** part of the blended cell's own claim.

---

## 4. Tracing — aarch64

Declared 2026-08-28 (PR #683). Evidence: `tracing/EVIDENCE-2026-08-28.md`.

**Concurrent userspace processes — partially uncheckable, disclosed.** The harness
(`scripts/test_tracing_via_gdb.sh`) boots a kernel built with `--features boot_tests`
(not the zero-feature production profile TTY uses), lets it free-run for a settle
window (20s in the cited run), then halts it with a GDB attach and dumps the trace
buffer — it does not stop the kernel at a defined boot stage. The evidence doc's own
citation (`tracing/EVIDENCE-2026-08-28.md` §2, `serials/aarch64-bootgate-markers-…`)
shows only `[BOOT_TESTS:TOTAL:109]` / `[TESTS_COMPLETE:109/109]` — the kthread-based
in-kernel test registry (`kernel::test_framework::executor::run_all_tests()`, whose
own doc comment says it "spawns kthreads to run tests in parallel" —
`kernel/src/test_framework/executor.rs:1-4`, current tree). **Those 109 tests run as
kernel threads sharing the kernel's own address space, not as separate userspace
processes** — confirmed directly: `run_all_tests()`'s body contains no call to
`create_user_process` or `spawn(` anywhere (grepped against the full function body,
current tree; 0 occurrences). That much is a checked fact, not an inference, and is
exactly what `tests/green_program_envelope_structure.rs`'s
`boot_tests_registry_stays_kthread_only` guards (§ Detector, item 4).
What is **not** checked, and is disclosed here rather than assumed: after the
kthread registry completes, the same boot flow continues into
`launch_init_from_elf()` and the ordinary production `init.rs` sequence described in
§1 above (`kernel/src/main_aarch64.rs`, the `#[cfg(feature = "boot_tests")]` block at
the top of `kernel_main` advances to `TestStage::ProcessContext` once the designated
init process exists, then falls through to the same userspace launch path a
zero-feature boot uses). A companion arc's own evidence shows this concretely:
`docker/qemu/run-aarch64-full-test.sh --rebuild`, built the same `boot_tests` way,
reaches **Phase 2** (shell-prompt detection, which needs `run_boot_script()` to have
executed) on this exact profile (`tty/EVIDENCE-2026-08-30.md` §8, the `#593` red).
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
into the ordinary production userspace-init sequence. `run-aarch64-full-test.sh`'s
own Phase-2 shell check (§4, `tty/EVIDENCE-2026-08-30.md` §8) is direct evidence that
*this exact gate script* reaches userspace init in the same boot the device census
also reads. The service-sequence gate's 50/50 GREEN run
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
   change to any of those 7 scripts.
2. **Two of the six standing cells (Tracing-aarch64, Bus/NIC-aarch64) were measured
   on the `boot_tests` feature profile, not the zero-feature production profile the
   other four (TTY, all three views) were measured on** — and the `boot_tests`
   profile's own boot flow continues into the same userspace-process sequence TTY's
   envelope documents, on top of a 109-test kthread registry TTY's profile never
   runs at all. This document does not know, and does not claim to know, exactly how
   many userspace processes were alive at the moment either cell's measurement was
   taken — see §4/§5-6's "uncheckable" notes.

---

## Detector

`tests/green_program_envelope_structure.rs` turns four of the claims above into a
host-side structural test (`cargo test --test green_program_envelope_structure`),
run the same way every other `*_structure.rs` ratchet in `tests/` is run — no kernel
build or QEMU boot required, since every check parses source/script text directly.
It is **not** a CI gate (this repo has no GitHub Actions and no git hooks are in
scope per the task); it is a test the sweep process — or any future arc touching
`init.rs`, the seven cited gate scripts, or `executor.rs` — is expected to run before
declaring or re-declaring a cell, the same way `tty_oracle_structure.rs` is already
run before every TTY declaration.

**What it checks (census-shaped — every check reads the current fact out of the
source it governs, never a hand-typed expected value pulled from this document):**

1. **TTY concurrency invariant.** Parses `userspace/programs/src/init.rs::main()`'s
   call sequence and classifies each helper it calls, before the arch-appropriate
   `run_tty_oracle()` call site, as *persistent* (contains `spawn(`/`spawnv(` with no
   matching `waitpid(` in the same function body — i.e. still running when the next
   call starts) or *reaped* (both, sequential). Asserts the aarch64 persistent count
   is exactly 1 (`heartbeat`) and the x86 persistent count is exactly 0, matching §1
   and §2 above. A future PR that adds a second background daemon before either
   arch's `run_tty_oracle()` call — the same shape of change #713 made to Filesystem
   — reddens this test by name instead of silently changing the concurrency envelope
   TTY was proven under.
2. **ext2 read-only/writable split.** Parses the ext2 `-drive` (or `drive_opts=`)
   declaration out of all 7 gate scripts named in §1-6 and asserts x86 scripts carry
   `readonly=on` and aarch64 scripts do not. Flags the moment any of the two x86
   scripts drops `readonly=on` (a real widening of the x86 write-envelope) or either
   direction drifts from what this document claims.
3. **`-smp` census.** Parses the `-smp N` value adjoining each gate script's own
   arch-marker line (`-M virt,gic-version=3` for aarch64, `-machine pc,accel=tcg` for
   x86) for the same 7 scripts and asserts aarch64 is 4 and x86 is 1, matching every
   CPU-count claim above.
4. **kthread-only boot_tests registry.** Parses
   `kernel/src/test_framework/executor.rs::run_all_tests()`'s body and asserts it
   calls neither `create_user_process` nor `spawn(` — the fact §4/§5-6 lean on to
   say the 109-test registry itself never becomes a second concurrent userspace
   process, whatever the surrounding boot goes on to do.

Each of the four is proven non-vacuous the same way `tty_oracle_structure.rs`
proves its own census functions non-vacuous (in-memory string mutation, no on-disk
edit, no rebuild): a **positive** mutation modeled directly on the #713 pattern (an
extra persistent aarch64 spawn inserted before `run_tty_oracle()`; `readonly=on`
stripped from an x86 script; an `-smp` value changed on a watched script; a
`create_user_process` call inserted into `run_all_tests()`'s body) is asserted to
flip the corresponding check from green to red, and a **control** mutation to
something the check does not own (an unrelated print-string edit inside a reaped
launcher; an `-smp` edit on a gate script none of the 6 cells cite; a call inserted
into a different function in the same file) is asserted to leave the check
unaffected. All eight mutation assertions pass against the current tree — see
`tests/green_program_envelope_structure.rs`'s own test functions for the exact
before/after pairs; nothing here restates them as a second copy that could drift
from the code.

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

---

## Claim-discipline self-check

Per the task's own instruction, this document was grepped for
`every|all|zero|always|never` before finishing: 67 hits (as of this final pass; the
count above was regenerated after the edit that added this sentence, not left stale
from an earlier draft). Each was read in context and falls into one of the patterns
below — a direct quote, a literal cited count, or a scoped/disclosed statement rather
than an unbacked universal; representative examples of each pattern are listed, not
all 67 individually:

- "**never** overlap with bsshd's own ext2 reads" (§2) — a direct quote from
  `userspace/programs/src/init.rs`'s own doc comment, attributed as a quote, not
  this document's claim.
- "**zero** ext2 reads or writes" (§1), "**zero** `target_arch` occurrences" (§2, a
  literal re-counted table cell), "run_all_tests()… 0 occurrences" (§4) — each is a
  direct grep/count result stated next to its method, not an unqualified universal.
- "no test in this program boots both arches at once" (§3) — scoped to "this
  program," not a claim about what's possible.
- "This document does not know… **never** asserts a process count" (§4) — a
  disclosure of a limit, not a universal claim about the world.
- "**zero**-feature production profile" (throughout) — a proper noun (the profile's
  own name in this codebase, `cargo build` with no `--features` flag), not a
  quantifier.
- Detector section: "asserts… neither… nor," "leave the check unaffected," "does not
  run against `main` on a schedule — nothing in this repository does" — each is
  either a specific assertion inside a named test function (checkable by reading
  that function) or a scoped negative about this repo's own CI/hook surface
  (verified: no `.github/workflows`, no git hooks referenced anywhere in this task).
