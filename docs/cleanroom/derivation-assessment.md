# Derivation assessment (ruling DEV-36(1))

27 Nov 2026. Tree: origin/main at 368e8c36 (#3246 merged, so the six verbatim GPL
copies are already gone from the tree).

DEV-35 keeps the historical citations in our docs and comments. DEV-36 adds one
question: does any of our *current* code look like it was substantively derived
from another system's source, and not just named or compared with it? This
document answers that question for each area the copyleft scan from #3242
touches. It also covers the comments in current kernel code that say they
"follow", "match" or "mirror" another system, which the scan's citation rules
did not catch.

How to read it:

- **Part A** gives the verdicts and their evidence. It is for the coordinator and
  the reviewer.
- **Part B** has the neutral requirements briefs. A clean-room rewriter reads
  **only** the Part B section for their area, together with the specifications it
  names and the area's interface and callers (DEV-36(2)). They do not read Part A,
  the old function bodies or the citing docs.
- **Part C** maps each area to where its work happens.
- **Appendix** places each of the scan list's 98 files into an area.

## Method

Inputs used, and only these:

- the scan list (`copyleft-scan-pr3242.md`: 350 lines in 98 files, with the
  introducing commits);
- git history: commit messages, dates, `git show --stat`, and our own diffs and
  commit trees;
- the prose of our own planning docs that cite other systems;
- our current code at origin/main.

What was not opened:

- the six GPL files under `docs/artifacts/turn1-artifacts/` (#3246 has since
  removed them);
- any outside source or web page.

Each citing doc was read through a filter that prints prose lines only. It drops
fenced blocks, indented code, and any line whose inline code spans look like
statements (`;`, braces, `=`, `->`, `&&`, calls with arguments). It marks each
drop as `[skipped N code line(s)]`, so no quoted code from another system was
shown to the assessor. Doc citations below are `path:line` into our own tree at
368e8c36, unless a commit is named.

Verdicts:

- **SUBSTANTIVE**: rewrite. Each such area gets a Part B brief.
- **UNCLEAR**: the coordinator decides. A brief is included in case the ruling is
  SUBSTANTIVE.
- **TRIVIAL/ACCIDENTAL**: a name, a constant, a UAPI fact, a comparison note, a
  behaviour any implementation would show, or a bug found by comparison and fixed
  our own way. No action.

Calibration rule (from the brief): a sequence that the governing specification or
hardware manual dictates, which any implementation written from that document
would share, is not derivation, even when a commit says it "matches" another
system.

## Summary

| # | Area | Verdict | Action and owner |
| --- | --- | --- | --- |
| A1 | Waitqueue sleep/wake primitive (`task/waitqueue.rs` and its scheduler hooks) | **SUBSTANTIVE** | Clean-room rewrite inside **E2** (park contract), brief B1 |
| A2 | Completions (`task/completion.rs`) | TRIVIAL | No action. E2 replaces it anyway |
| A3 | ARM64 idle/WFI gate and reschedule IPIs | TRIVIAL | No action. E9 owns this code |
| A4 | GICv3 and the ARM generic timer | TRIVIAL | No action. E9 owns this code |
| A5 | Scheduler wake and dispatch (other than A1's hooks) | TRIVIAL | No action. E9 owns this code |
| A6 | PCI MSI and xHCI | TRIVIAL (already remediated) | No action: e63a5ed6 (#1695) and 91d5f17e (#1743) |
| A7 | AHCI and block queuing | TRIVIAL | No action. E4 owns this code |
| A8 | TCP | TRIVIAL | No action. E10 owns this code |
| A9 | Legacy virtio-gpu PCI driver (`drivers/virtio/gpu_pci.rs`) | **SUBSTANTIVE** (no longer reached at run time) | **Standalone PR** that removes the module and its dead branches, brief B2 |
| A10 | virtio-net PCI | TRIVIAL | No action. E10 owns this code |
| A11 | Signals | TRIVIAL | No action |
| A12 | SysV and POSIX IPC limits | TRIVIAL | No action |
| A13 | Softirqs and bottom-half accounting (with kthreads and workqueues) | **UNCLEAR** (softirq part only) | Coordinator. Brief B3 if ruled SUBSTANTIVE |
| A14 | PTY and terminal job control | TRIVIAL | No action |
| A15 | procfs | TRIVIAL | No action |
| A16 | Process, wait, epoll, sockets, user copies, ELF loading | TRIVIAL | No action |
| A17 | GPU (GK208/GT 710) | TRIVIAL (permissive sources) | No action |
| A18 | QEMU fork docs | Out of scope | No action |
| A19 | Agent guidance files | Fixed | #3246 (DEV-35(3a)) |

## Part A: verdicts and evidence

### A1. Waitqueue sleep/wake primitive: SUBSTANTIVE

Signal 1 (a design doc walks the other system's code, then adopts its approach):

- `docs/planning/f32-waitqueue/design.md:14-16` says the replacement "should look
  more like" the other system's waitqueues. Lines 18-42 then set out that system's
  two structures and the semantics of its prepare, wake and finish routines. Our
  design follows from that description (lines 125-134 and 159-216), using a TID
  queue in place of intrusive entries.
- `docs/planning/f32c-waitqueue/ordering-audit.md:18-38` sets the cited source
  side by side with ours, routine by routine, by file and line. Lines 60-80 frame
  the fix as restoring that system's "ordering invariant" and finish "as in
  Linux".
- `docs/planning/f32e-linux-prepare-to-wait/audit.md:31-46`: "shrink the waitqueue
  critical section to the exact Linux shape". Each of its four steps cites the
  routine it mirrors: the state-publish barrier, the same-lock wake path, and the
  schedule-side state check.
- `docs/planning/f32f-immediate-wake/decisions.md:4` scopes that run to "Linux-parity
  immediate task-context waitqueue wake".

Signal 2 (a commit or comment says the code matches or mirrors another system):

- df914fbe (2026-04-19): "F32c–F32j: Linux-parity waitqueue + idle gate + CPU0 SGI
  admission fix (#328)".
- `kernel/src/task/waitqueue.rs:3`: "Breenix's equivalent of Linux waitqueues".
  Lines 44-47: "The public semantics match Linux's core pattern". Line 152: the
  finish routine "mirrors" the other system's.
- `kernel/src/task/scheduler.rs:6257-6260`: the publish fence is justified by the
  other system's state-setting primitive. Lines 6374-6378: the task-context wake
  helper "Mirrors" the other system's waitqueue-to-wake nesting.
- `kernel/src/task/completion.rs:116-118` and `kernel/src/task/waitqueue.rs:152`
  name the other system's wait-finish routine.

Signal 3 (our code follows the cited structure step for step):

- The current entry points carry the other system's internal, non-UAPI routine
  names (`kernel/src/task/waitqueue.rs:65, 96, 154, 163, 175, 197`).
- The step order that the F32c/F32e docs prescribe is still the code's shape at
  368e8c36:
  - enqueue under the queue lock, then publish the blocked state in the same
    critical section (`waitqueue.rs:72-85`, `96-137`);
  - a full fence after publication (`scheduler.rs:6257-6260`, inside
    `block_current_for_io_publish` at 6203);
  - wakes taken under the same queue lock and routed to the scheduler
    (`waitqueue.rs:175-221`, `344-350`; `scheduler.rs:6379`);
  - a schedule-side recheck of the state (`waitqueue.rs:438` onwards);
  - a finish step that removes the entry and restores the state
    (`waitqueue.rs:154-170`).

Mitigating facts:

- The data structure is ours: a duplicate-free TID `VecDeque` behind a spin mutex,
  not intrusive entries (`design.md:125-134`).
- Later work is ours: reservation before publication (27b3e8d4, `waitqueue_alloc.rs`),
  the deferred and IRQ try-lock wakes, x86 halting waiters (5c168333, d591a980)
  and the task-owned inbox (9520a3b0).
- "Register, then recheck the condition, then sleep" is the textbook way to avoid
  a lost wake (Lampson and Redell 1980; Birrell 1989).

Even so, the docs prescribe the shape routine by routine from another system's
source, and the comments still say "mirrors". That meets each of the three DEV-36 signals.
E2 replaces this primitive with a park contract anyway, so the rewrite costs
little there.

### A2. Completions: TRIVIAL

- 7aa8334f (2026-03-25) says AHCI completion is "matching Linux's wait_for_completion
  pattern". "Wait until the operation is done" is generic.
- Our mechanism is ours:
  - a token-matched done word plus one waiter TID (`kernel/src/task/completion.rs:327-360`, `733`);
  - generation counters (80f09aa1);
  - killable waits (9e0dd9b4);
  - the take-back of `BlockedOnIO` for #1244 (fe5a83bd).
- The F32h design that would have adopted the other system's IRQ completion path
  (`docs/planning/f32h-ahci-wake/proposal.md:127-195`, Option C) is not in the tree at 368e8c36.
  Interrupt wakes now go through our task-owned inbox (9520a3b0, R17).
- The comments at `completion.rs:117`, `548` and `678` are comparison notes.

### A3. ARM64 idle/WFI gate and reschedule IPIs: TRIVIAL

- `docs/planning/f32i-cpu0-wfi-wake/linux-audit.md` walks the other system's idle
  loop, barriers, IPI dispatch and SGI send by file and line. `proposal.md:20-38`
  then proposes "a real Linux-style sleep gate", and F32j built it
  (`f32j-idle-sgi-admission/exit.md:29-51`).
- The gate as built (`kernel/src/arch_impl/aarch64/context_switch.rs:7274-7388`) is:
  mask DAIF, check `need_resched` and pending wakes, `dsb sy`, `wfi`, unmask, check
  again. The Arm ARM dictates that sequence. WFI wakes on a pending interrupt even
  while PSTATE masks it (Arm ARM, "Wait for Interrupt"), so the only lost-wake-free
  gate is check-with-interrupts-masked, then WFI. Any implementation written from
  the manual has these steps. The comments at 7278, 7308, 7315, 7334 and 7382 are
  comparison notes.
- The one design that would have followed the other system's structure, a
  target-CPU wake list driven by a function-call IPI (`proposal.md:52-80`; F32k), is
  not merged. Branch `f32k-ttwu-input-cursor` (91aa574d, 8cb108a9) is not an
  ancestor of origin/main, and `docs/planning/f32k-validate/exit.md:9` records
  "FAIL. Do not merge."
- F16 found that our ISR wake path sent an SGI to each online CPU that looked idle
  (`docs/planning/f16-scheduler-audit.md:156-183`). The fix removed that scan
  (lines 185-209). That is a bug found by comparison and fixed our own way.

### A4. GICv3 and the ARM generic timer: TRIVIAL

The GIC work:

- The sweeps compared register values and orders with the other system's GIC
  driver (`docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md:2786-2799, 2880-2892,
  2990-3002, 3087-3105`).
- The results are spec-dictated programming:
  - redistributor discovery walks 128 KiB RD frames, matches `GICR_TYPER`
    affinity, and stops at `GICR_TYPER.Last` (`gic.rs:1484-1490`);
  - each used SGI is enabled in `GICR_ISENABLER0` (`gic.rs:1659-1686`);
  - SRE/DFB/DIB, EOImode and IGRPEN1 are set.
  The GICv3/v4 architecture specification (IHI 0069) defines each of these.
- `LINUX_DEFAULT_PMR = 0xF0` (`gic.rs:238-239`) is a constant.
- The SGI-enable fix was a bug: SGI/PPI lines were left disabled
  (`f32j-idle-sgi-admission/exit.md:53-63`). It was found by comparison and fixed
  by enabling the SGIs we use.

The timer work:

- 68dc0be6 (2026-03-28), "use IMASK-based timer control matching Linux", is the
  example DEV-36's own brief names.
- Its content is the generic timer's register protocol:
  - set `CNTV_CTL_EL0.IMASK` at handler entry;
  - re-arm by writing an absolute `CVAL`, then `CTL = ENABLE`;
  - leave ENABLE set.
  The code is `timer_interrupt.rs:425-440`, `528-570` and `705-720`.
- The Arm ARM's generic-timer chapter defines ENABLE, IMASK, ISTATUS and CVAL. The
  hypervisor behaviour that forced the choice was found from our own captures (the
  commit body).
- Choosing to mask rather than disable is one bit among the three acknowledge
  methods the manual offers. That is not substantive expression.
- The comments at `timer_interrupt.rs:432`, `533`, `557` and `712` are comparison
  notes. `docs/planning/f20e-audit-bisect/exit.md:68-72` likewise only notes that
  the other system writes CVAL before CTL.

This is the closest call in the TRIVIAL set. E9's tickless one-shot deadlines
rewrite this file anyway.

### A5. Scheduler wake and dispatch: TRIVIAL

- bedda648 (2026-03-27) says "matching Linux's cpu_switch_to pattern". The change
  makes kernel-to-kernel switches restore callee-saved registers and `ret`, instead
  of `eret`, so DAIF is not restored from a stale SPSR (commit body). That is the
  generic callee-saved context switch, found in any OS text and in permissively
  licensed teaching kernels. The comments at `context_switch.rs:1521` and `7022`
  and `thread.rs:822` are comparison notes.
- `scheduler.rs:2118` (`on_cpu`), `4666`, `5893`, `7058` and `8114` compare an
  invariant or a behaviour, which is observable or POSIX-defined.
- `scheduler.rs:6257` and `6376` belong to A1.

### A6. PCI MSI and xHCI: TRIVIAL (already remediated)

- 5780377f (2026-04-21, "Linux-order PCI MSI programming") rewrote
  `Device::configure_msi` as a ten-step sequence that cited the other system's MSI
  source by line (`docs/planning/f32t-xhci-msi/linux-ground-truth.md:44-81`). That
  was SUBSTANTIVE when written.
- e63a5ed6 (2026-10-17, #1695, closed) re-derived it from PCI Local Bus 3.0
  §6.8.1, with constants named for the specification's fields. It dropped the
  other-kernel citation and three uncalled helpers. The current function
  (`kernel/src/drivers/pci.rs:364-429`) cites only the specification. The order
  that remains (MSI off, mask, program address and data, INTx off, enable,
  unmask) is the one the capability's own rules imply.
- The ARM64 USB stack written against the other system was retired unread in
  91d5f17e (2026-10-18, #1743). A clean-room xHCI/HID driver replaced it
  (1e7c1496). The tracked `ehci-hcd.c` copy (361c5a6c) had no live consumer, and
  `kernel/src/drivers/usb/` no longer exists.
- `docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/` only records
  that history.

### A7. AHCI and block queuing: TRIVIAL

- `docs/planning/f32h-ahci-wake/audit.md` walks the other system's AHCI interrupt,
  block-completion and wake paths. It concludes with a design that is not in the
  tree at 368e8c36 (A2).
- `docs/artifacts/turn53-artifacts/p12-ahci-classification.md:92-120, 198-204`
  and `docs/polling-allowlist.md:85-115` cite the other system only as
  "precedent" for keeping a bounded poll. The polls are the AHCI 1.3.1 engine
  handshakes: clear `PxCMD.ST`, wait for `PxCMD.CR`, the FR wait and the
  BSY/DRQ readiness check.
- `ARM64_CPU0_SMP_INVESTIGATION.md:3250-3265` found our edge-sampled completion
  missing slots. The fix derives completion from `PxCI`, which is how AHCI 1.3.1
  defines command completion. That is a bug found by comparison.
- `kernel/src/drivers/ahci/mod.rs:194` (a 5 s timeout), `1325`, `1903`, `1952`,
  `2995` and `3330` are a constant and comparison notes.
- No block-layer structure from the other system exists in the code. A
  `git grep` for its block-layer identifiers returns only those comments.
- R2's bounded retirement (#3235) and E4's queued block layer rewrite this driver
  from AHCI 1.3.1 anyway.

### A8. TCP: TRIVIAL

- `kernel/src/net/tcp.rs` is laid out by RFC 9293, 6298, 6528, 6056 and 2525, with
  those sections cited inline.
- The other system is named for behaviour and defaults:
  - the errno per state on RST (`tcp.rs:20-23`, `1392`);
  - TIME_WAIT and FIN_WAIT_2 times, the backlog cap and RTO bounds (`154-175`);
  - keepalive defaults (`177-180`);
  - the zero-window and keepalive probe form (`1532`, `1739`);
  - the SO_REUSEADDR conflict rule (`1911`);
  - the `/proc/net/tcp` columns (`1989-1992`);
  - POLLOUT's half-free rule (`2146`);
  - SYN-ACK retries (`2660`).
- Each of these is a documented tunable or user-visible behaviour (tcp(7),
  socket(7), proc(5)) or a single constant. They name behaviour; they do not set the structure of our code.
- E10 rewrites congestion control, timers and tables from RFCs anyway.

### A9. Legacy virtio-gpu PCI driver: SUBSTANTIVE (no longer reached at run time)

Signal 1:

- fb9d81ef (2026-05-19, #343, "Make virtio-gpu completion interrupt-driven")
  rewrote 1,049 lines of `kernel/src/drivers/virtio/gpu_pci.rs`. The same commit
  added six verbatim GPL source files of the other system's virtio-gpu and
  virtio-PCI drivers (paths in `git show --stat fb9d81ef`). Its doc
  `turn1-linux-probe-virtio-gpu.md` (in that commit's tree) maps that driver's
  routines in its section C, lines 110-131. Section E, line 173, proposes to
  "Implement the Linux-shaped interrupt path first".
- `turn2-implementation.md:7` (same commit) reports "The Linux-shaped MSI-X routing
  is implemented".

Signals 2 and 3 (`gpu_pci.rs` at 368e8c36):

- 529-532: per-command DMA storage, where "The active VirGL present path mirrors
  that model".
- 999-1001 and 1023: "the same two-vector layout Linux uses" and "Linux-shaped
  vectors".
- 1253: MSI-X before queue setup, following the other driver.
- 1279, 1331, 1348 and 1394: each bring-up step names the other driver's routine
  it follows.
- 3314: separate header and payload buffers, "matching Linux's".

Some of that order is also what VirtIO §3.1.1 and §4.1.4 require. But the comments
present each step as the other driver's, and the source files sat in the commit
that wrote them.

Why this is a removal and not a rewrite:

- c779be1f (2026-10-20) gave Parallels' virtio-gpu device to the GPU core
  (`kernel/src/drivers/mod.rs:321-322`). QEMU's virtio-gpu-gl-pci already went to
  the core (`drivers/mod.rs:264-265`; 363ce41b).
- `gpu_pci::init` (`gpu_pci.rs:1206`) has no caller at 368e8c36.
  `GPU_PCI_INITIALIZED` is set only inside it (`gpu_pci.rs:1567`), so the
  external branches guarded by `gpu_pci::is_initialized()` (B2 lists them) are
  dead. Its interrupt dispatch does not match, because `get_irq()` and `get_config_irq()` read state
  that only `init` fills.
- The live driver (`kernel/src/gpu/virgl/queue.rs:1-6` and siblings) was built from
  VirtIO 1.3 CSD01 and cites its sections.
- The remedy is to delete the legacy module and its dead branches: brief B2. Its
  VirGL helper `drivers/virtio/virgl.rs` has no other user (`gpu_pci.rs:3399`,
  `4019`, `4252`).

### A10. virtio-net PCI: TRIVIAL

- acb62bab (2026-05-20) calls the IRQ-to-NetRx flow "Linux-shaped".
  `kernel/src/drivers/virtio/net_pci.rs:1229-1272` follows the other system's
  callback re-enable "shape".
- The steps are:
  1. clear `VRING_AVAIL_F_NO_INTERRUPT`;
  2. fence;
  3. reread `used.idx`;
  4. suppress again if work raced in.
  That is VirtIO's own used-buffer notification suppression protocol (VirtIO 1.2
  §2.7.7, and §2.7.10 for the event-index form), which any driver must follow to
  avoid a lost notification.
- "Interrupt off, then a budgeted poll in a softirq" is the published design of
  Salim, Olsson and Kuznetsov, "Beyond Softnet" (USENIX ALS 2001).
- E10's modern virtio-net rewrites this file anyway.

### A11. Signals: TRIVIAL

- 913d14ae (2026-10-09, "Deliver signals with Linux siginfo, ucontext and FP state")
  is ABI work. The frame, `siginfo`, `ucontext`, `stack_t` and `sigaction` layouts
  (`signal/types.rs:9`, `128`, `175`; `signal/delivery.rs:552-557`, `1053-1058`)
  are UAPI facts that Linux-ABI binaries require.
- The behaviours named by the other system's internal routines are documented or
  observable:
  - a blocked or ignored fault signal is unblocked and reset
    (`delivery.rs:300-307`; sigaction(2), signal(7));
  - synchronous signals are taken first (`constants.rs:168`);
  - stop handling (`delivery.rs:1542`);
  - group exit status (`1727`);
  - the protected-init drop (`docs/planning/teardown-unification/DESIGN.md:1224-1236`,
    `PLAN.md:2634-2640`; kill(2): only signals init has handlers for reach it).
- The teardown design's structure (tranches and registers) is ours.

### A12. SysV and POSIX IPC limits: TRIVIAL

- `kernel/src/ipc/mqueue/store.rs:11-32` and `kernel/src/ipc/mqueue.rs:69-75` and
  `668` hold limits and magic numbers. `ipc/sysv/*.rs` hold the `IPC_INFO`/`*_STAT`
  commands. These are UAPI facts documented in mq_overview(7), msgctl(2), semctl(2),
  shmctl(2) and statfs(2), which DEV-34(4) allows.
- "Checked in Linux's order" (`mqueue/calls.rs:301`, `store.rs:468`,
  `ipc/sysv/sem.rs:91`) describes the black-box errno precedence.

### A13. Softirqs and bottom-half accounting: UNCLEAR (softirq part)

Facts:

- `docs/planning/KTHREAD_IMPLEMENTATION_PLAN.md:495-513` lists the other system's
  kthread, workqueue, softirq and reclaim source files (and the FreeBSD kthread
  files) as references. It has no line-level citations and no routine-by-routine
  walk.
- edf9186f (2026-01-21) is "Linux-style softirq subsystem ... following Linux
  patterns".
- `kernel/src/task/softirqd.rs:10-14`: "This implementation follows Linux's
  softirq architecture". Lines 41-72 hold a fixed set of ten softirq types "matching
  Linux priorities" in that system's order, and a restart limit of 10, "we match
  that".
- `kernel/src/arch_impl/aarch64/constants.rs:191-231` and `kernel/src/per_cpu.rs:183-190`
  give the preempt-count partitioning as "Matches Linux kernel's exact bit
  partitioning". That includes the convention that the low softirq bit tells
  execution apart from disable nesting (`constants.rs:215-220`).

Against derivation:

- The softirq design (a fixed bitmap of deferred-work classes, a restart cap, then
  a per-CPU thread) is described in published texts and in the other system's
  prose documentation. No source walk is on record.
- The code itself is a small pending bitmap, a handler table and a thread
  (`softirqd.rs:157-360`).
- The kthread API (9f67bb79) and the workqueue (78d85aff) reuse a few routine
  *names* (`kthread_should_stop`, park/unpark) over generic designs. By DEV-36's
  definitions that part is TRIVIAL.

Why UNCLEAR: the whole architecture, including the enumeration's order and the
counter layout, was adopted by name from one system, and the commit and comments
say so. The material that shows how it was learned is a reading list. If the
coordinator rules SUBSTANTIVE, brief B3 applies. The natural home is E9 (per-CPU
dispatch), or a standalone PR.

### A14. PTY and terminal job control: TRIVIAL

- `docs/planning/PTY_IMPLEMENTATION_PLAN.md:846-851` is a references list (the
  other system's and FreeBSD's PTY sources, POSIX and "The TTY Demystified").
- The code is POSIX's General Terminal Interface plus documented behaviour.
  `kernel/src/tty/jobctl.rs:5` names the other system's access-check routine for
  the tcsetpgrp/SIGTTOU rules, which POSIX XBD §11.1.4 and tcsetpgrp(3) state.
  `ctty.rs:170-339` and `pty/io.rs:35` are observable behaviours.

### A15. procfs: TRIVIAL

`kernel/src/fs/procfs/pid_stat.rs:4` (`USER_HZ`) and the `/proc/<pid>/stat` and
`/proc/net/tcp` formats are proc(5) facts.

### A16. Process, wait, epoll, sockets, user copies, ELF loading: TRIVIAL

- `process/manager.rs:2273` and `3217`, `process/process.rs:284` and `1092`, and
  `syscall/wait.rs:5` name routines and flags for POSIX or observable behaviour.
- `syscall/epoll.rs:57` and `496` are a constant and a behaviour.
- `syscall/socket.rs:28-59` and `190` are limits and the errno order.
- `socket/opts.rs:45`, `100` and `106`; `socket/tcp.rs:9`, `105` and `161`; and
  `socket/types.rs:27` and `102` are UAPI and behaviour.
- `syscall/userptr.rs:320` compares how far a faulting user copy reports, which
  is an observable result.
- `syscall/fs.rs:101` names a published UAPI header, which CLAUDE.md allows.
- `elf.rs:557` and `653` and `aarch64/elf.rs:336` and `534` ("Linux-style physical
  memory access") describe a generic choice.
- The userspace and test hits (`signal_test.rs:92`, `suite_memory.rs:155`,
  `suite_time.rs:144`, `libc-suite/abi_check.c:105`, `libc_sched_cpuset_host.py:76`,
  `stdio-host/support.rs:275`, a clock-number translation) are UAPI facts.

### A17. GPU (GK208/GT 710): TRIVIAL (permissive sources)

- Each of the 13 GPU files in the scan (appendix, row A17) cites nouveau's MIT-licensed files, Mesa (MIT) or VMware's
  interface headers. The docs record the licence beside the citation
  (`docs/gui/gpu.md:12-15`, `1067-1070`; `docs/gui/e22-2-u9b.md:33-36`;
  `docs/gui/gt710-c3a.md:16-18`).
- CLAUDE.md allows these sources. THIRD_PARTY_NOTICES carries their grants.
- `kernel/assets/kepler/manifest.json` and the `docs/gui/provenance/*.json` hits
  record the pinned tree and prefix of nouveau firmware sources.

### A18. QEMU fork docs: out of scope

`docs/planning/arm64-catchup/a3-*.md` and `a4-u7-ui-testing-baseline.md` analyse
host-side QEMU behaviour for our separately built GPL QEMU fork (DEV-34(3) keeps
it as a separate program). No Breenix code is built from them.

### A19. Agent guidance files: fixed

The scan's `.claude/commands/os-research.md`, `.claude/skills/os-research/SKILL.md`
and `.agents/skills/breenix-interrupt-syscall-development/SKILL.md` hits are the
guidance DEV-35(3a) fixed. #3246 is merged at 368e8c36.

## Part B: neutral requirements briefs

Each brief stands alone. The rewriter reads only their brief, the specifications
it names, the public signatures and callers it lists, and the tests it names. They
do not open the existing function bodies of the files being replaced, the planning
docs under `docs/planning/f16-*`, `f32*` or `f20e-*`, or Part A.

### B1. Thread sleep and wake primitive (inside E2)

**Purpose.** Kernel code must be able to put the current thread to sleep until an
event that another thread, another CPU or an interrupt handler signals, with:

- no lost wakeups;
- a bounded timeout;
- interruption by signals or kill;
- several event sources watched at once (poll, select, epoll);
- no allocation on any path that cannot fail cleanly.

E2 defines a park contract for sleepable contexts. This primitive is its
event-queue half.

**What it replaces.** `kernel/src/task/waitqueue.rs` (the `WaitQueueHead` type and
its free functions), plus the two scheduler hooks it uses:

- the blocked-state publication in `kernel/src/task/scheduler.rs`, function
  `block_current_for_io_publish` (6203), reached through
  `block_current_for_io_with_timeout` (6288; x86 run-queue variant at 2506);
- the task-context wake, `Scheduler::wake_waitqueue_thread` (6379) and the free
  function `wake_waitqueue_thread` (9344).

The interrupt-context wake `isr_unblock_for_io` (10388) and the task-owned inbox
behind it are ours (R17) and stay. The new primitive calls them through their
signatures.

**Current interface used by callers.** Names are the rewriter's to choose. Callers
are updated in the same change. Site counts are `git grep` text matches of the
call outside `waitqueue.rs` at 368e8c36. A few may be same-named methods of other
types.

| Operation today | Contract | Representative callers |
| --- | --- | --- |
| `WaitQueueHead::new()` (const) | A static, empty queue | The type is named in 43 files besides its own. For example `kernel/src/drivers/ahci/mod.rs` (`PORT_IO_WAITERS`), `kernel/src/gpu/mod.rs` (`WAITERS`) |
| `prepare_to_wait(state) -> Option<tid>` | Register the current thread and publish it as blocked-on-I/O. Returns no TID if it cannot | `kernel/src/drivers/nvme/mod.rs:264`, `kernel/src/drivers/virtio/block.rs:89` (6 sites) |
| `prepare_to_wait_checked(state, deadline, cond) -> {Mismatch, Queued, PublishFailed, OutOfMemory}` | Evaluate `cond` atomically with registration against this queue's wakers. Register and publish only if `cond` holds | `kernel/src/block/cache_kernel.rs:73`, `kernel/src/console/mod.rs:452` (31 sites) |
| `schedule_current_wait()` | Sleep while still published blocked. Return when made runnable. On ARM64 it records the caller's return address for lockup captures | `kernel/src/block/cache_kernel.rs:78`, `kernel/src/drivers/ahci/mod.rs:499` (16 sites) |
| `halt_blocked_current()` (x86) | Halt a published-blocked thread, yielding first if others are ready | `kernel/src/drivers/xhci/mod.rs:1000`, `kernel/src/serial.rs:223` |
| `finish_wait()` / `finish_wait_for(tid)` | End a wait however it ended: deregister, return the thread to running if still published blocked, clear `blocked_in_syscall` | 39 / 5 sites, for example `kernel/src/gpu/mod.rs:2435` |
| `wake_up()`, `wake_up_one()`, `wake_up_n(max)` | Wake all, the oldest one, or up to `max` registered threads | 78 / 7 / 0 sites, for example `kernel/src/drivers/ahci/mod.rs:534` |
| `wake_up_deferred()` | Wake the registered threads without taking the scheduler lock, for callers holding the process manager | `kernel/src/fs/breenfs/commit_signal.rs:36`, `kernel/src/fs/ext2/mapped_io.rs:328` (19 sites) |
| `try_wake_up_n_from_irq(max)` | From hard IRQ with interrupts masked: try-lock only. Return 0 on contention and leave the queue intact | `kernel/src/gpu/mod.rs:2731` |
| `add_waiter(tid) -> bool`, `take_waiter(tid) -> bool`, `take_waiters(&mut [tid])`, `has_waiters()`, `contains_waiter(tid)` | Multi-queue waits register without publishing, publish once themselves, recheck, then deregister | `kernel/src/syscall/multiplex.rs:113`, `kernel/src/socket/wait.rs:77`, `kernel/src/gpu/mod.rs:283`, `kernel/src/console/mod.rs:474`, `kernel/src/ipc/stdin.rs:602` |

**Requirements.**

1. *No lost wake.* Suppose a waker makes the condition true and then issues any
   wake on the queue. Any thread whose registration became visible before that
   wake then either sees the condition in its post-registration check, or is made
   runnable. A thread that registered after the condition became true sees it in
   its check.
2. *Checked registration.* The `cond` evaluation, the registration and the blocked
   publication are one critical section with respect to that queue's wakers. A
   false `cond` leaves no registration and no published state.
3. *Allocation.* Capacity is reserved before anything is published. A failed
   reservation returns an out-of-memory outcome and leaves the queue and the
   thread unchanged. No wake path allocates.
4. *Interrupt context.* A wake from hard IRQ must not spin on a lock that task
   context holds with interrupts enabled. It hands the TID to the scheduler's
   interrupt wake (`isr_unblock_for_io`). The try-lock variant either wakes up to
   `max` or leaves the queue unchanged.
5. *Lock order.* A task-context wake may enter the scheduler while holding the
   queue lock. The scheduler must not take a waitqueue lock. The deferred wake must
   not enter the scheduler lock.
6. *Ending a wait.* After a wake, a timeout, a signal, a kill or a failed
   publication, the thread is no longer registered, its state is running, and
   `blocked_in_syscall` is clear. A wake that arrives after the thread is running
   has no effect.
7. *Wake semantics.*
   - One registration per thread per queue.
   - Waking one picks the oldest registration.
   - Waking `n` wakes at most `n` and returns the count.
   - A woken entry is removed only after the scheduler has accepted responsibility
     for that wake.
8. *Timeouts.* An optional absolute deadline (`wake_time_ns`) ends the wait. A wake
   and a timeout that race end the wait once.
9. *Preemption contract.* Syscall callers enter with preemption disabled. The
   sleep enables preemption before it can switch, and restores the entry
   invariant before it returns.
10. *Diagnostics kept.* ARM64 records the wait site's return address and the queue
    identity in the thread's wait-site record, and per-thread progress counters on
    each wait return (current uses: the lockup capture, 0d15c232 and e8bf5e81).
    The record's layout is ours to change, but the capture must still name the
    blocked site.
11. *x86 halting waiters.* A waiter halts with the blocked check redone under
    masked interrupts, so a wake landing after the check ends the halt.

**Governing specifications and literature.**

- Arm Architecture Reference Manual for A-profile (DDI 0487), section B2,
  "The AArch64 Application Level Memory Model": the barriers between publication
  and the condition check.
- Intel SDM Vol. 3A §9.2, "Memory Ordering": the x86 case.
- POSIX.1-2017 XSH §2.4, "Signal Concepts", and the EINTR and timeout rules of each
  blocking call built on this primitive: read, write, poll, select, sem_timedwait,
  mq_timedreceive. Also futex(2) for the futex wait.
- Lampson and Redell, "Experience with Processes and Monitors in Mesa", CACM 23(2),
  1980: a wake is a hint, and the waiter rechecks.
- Birrell, "An Introduction to Programming with Threads", DEC SRC Report 35, 1989.

**Deterministic tests**, each check-first, red on the code before the change that
makes it pass:

- A host model of the scheduler state (running, published blocked, runnable) and
  one queue. It enumerates each interleaving of the waiter's steps (register,
  check, sleep, finish) against the waker's steps (set condition, wake), and
  asserts requirement 1. Repeat for timeout against wake, signal against wake,
  wake-one with two waiters, and a waker in interrupt context.
- Out of memory at reservation: the outcome is out-of-memory, no state is published
  and no entry is registered.
- The IRQ try-lock under a held lock returns 0, and the queue still holds each
  entry it held before.
- Requirement 6 after each way a wait ends.
- Keep the queue unit tests that now live in `kernel/src/task/waitqueue.rs` (from
  line 495), reworded to the new names.
- The existing structure tests that pin callers' wait shapes must still pass, or
  be updated where they pin a name: `tests/pipe_fifo_blocking_structure.rs`,
  `tests/teardown_structure.rs`, `tests/durable_irq_wakes_test.py`,
  `tests/wait_epoch.rs` and `tests/timer_wake_dispatch_structure.rs`.

Per DEV-30, boots are only for what these cannot settle.

### B2. Remove the unreached legacy virtio-gpu PCI driver (standalone PR)

**Purpose.** Breenix keeps one virtio-gpu PCI driver: the GPU core in
`kernel/src/gpu/`. It already drives Parallels' device (`kernel/src/drivers/mod.rs:321-322`)
and QEMU's virtio-gpu-gl-pci (`drivers/mod.rs:264-265`). The older module
`kernel/src/drivers/virtio/gpu_pci.rs` and its helper `kernel/src/drivers/virtio/virgl.rs`
are deleted along with the branches that consult them (listed below). No target's run-time
behaviour changes: the old module's `init` has no caller, so its initialized flag
is false on each boot today.

**What is removed or reduced.** Branches that test `gpu_pci::is_initialized()`
collapse to their false arm. Calls to `gpu_pci::get_irq()` and
`get_config_irq()` go, since both return no IRQ today. The list:

- `kernel/src/drivers/virtio/gpu_pci.rs` (whole file) and `pub mod gpu_pci;`
  (`kernel/src/drivers/virtio/mod.rs:33`);
- `kernel/src/drivers/virtio/virgl.rs` and `pub mod virgl;` (`virtio/mod.rs:54`),
  whose only users are in `gpu_pci.rs`;
- `kernel/src/console/display/virgl_backing.rs` and its `Display::VirglBacking`
  variant (`kernel/src/console/display/mod.rs:11`, `34`, `128`, `179`, `195`);
- `kernel/src/console/mod.rs:492` (presentation-pending check) and `781` (display
  choice);
- `kernel/src/graphics/arm64_fb.rs`:
  - `init_gpu_pci_gop_framebuffer` (300) and its caller
    (`kernel/src/main_aarch64.rs:1156-1170`), keeping the pure-GOP fallback the
    caller already has;
  - the dimension and flush branches at 76, 116, 152-153, 169-170, 399-402,
    425-426, 439-440, 499-500, 520-521 and 531-532;
- `kernel/src/main_aarch64.rs:1132-1135` and `1213-1214` (the VirGL display
  choice, now false), and `2568-2570` (`gpu_mmio::init` becomes unconditional
  there, as it already is in effect);
- `kernel/src/arch_impl/aarch64/exception.rs:2555-2565` (the dispatch to the
  removed handlers). The GPU core's dispatch right after it stays;
- comments that name the module: `kernel/src/drivers/ahci/mod.rs:2857`,
  `kernel/src/interrupts/context_switch.rs:256`, `kernel/src/per_cpu.rs:1563`.

**Requirements.**

1. No new warnings on x86-64 and ARM64. No item made dead by the removal is
   left behind under an `allow`.
2. The GPU core's device claim, interrupt dispatch and console display paths are
   untouched.
3. The GOP and virtio-gpu-mmio display paths keep their current selection order
   on each target.

**Governing specification.** A removal needs no specification. The surviving driver
follows VirtIO 1.3 CSD01, §2.7 and §5.7.

**Deterministic tests.**

- A source-structure check, red before the removal and green after:
  - no file `kernel/src/drivers/virtio/gpu_pci.rs` or `kernel/src/drivers/virtio/virgl.rs`;
  - no `drivers::virtio::gpu_pci` path anywhere under `kernel/src`;
  - the PCI branch of `drivers::init` still hands virtio-gpu to `crate::gpu::init`.
- The fixture string at `tests/canvas_wait_test.py:129` names the module inside a
  synthetic source tree. Leave it alone, or update it if the test reads the real
  tree.
- Boots are not needed, because the removed code is unreached. If the coordinator
  wants run-time evidence, it is the end-of-program Parallels run (DEV-30(2)).

### B3. Deferred-work classes and bottom-half accounting (only if A13 is ruled SUBSTANTIVE)

**Purpose.** Interrupt handlers raise named classes of deferred work that run soon
after the interrupt, with interrupts enabled, ahead of ordinary threads. Under
sustained load the work moves to a per-CPU kernel thread, so user threads are not
starved. Code may hold off that deferred work on the current CPU, and the CPU's
nesting counter tells "running deferred work" apart from "deferred work held off".

**Interface and callers.** `kernel/src/task/softirqd.rs` (names are the
rewriter's):

- registration of a handler per class (157): 6 call sites, for example
  `kernel/src/net/mod.rs`;
- raise by class or number (171, 177): 27 sites, among them the NIC drivers
  (`drivers/bcm5720`, `drivers/e1000`, `drivers/virtio/net_*`, `drivers/rtl8139.rs`),
  `drivers/xhci/mod.rs`, `signal/timers.rs`, `net/rx_poll.rs`, `net/rx_claim.rs`
  and `socket/object.rs`;
- run pending work (197): 8 sites, including the interrupt exits
  (`kernel/src/per_cpu.rs`, `kernel/src/arch_impl/aarch64/exception.rs`) and
  `syscall/handlers.rs`;
- a pending query (185);
- initialization and the per-CPU daemons (323, 338), called from `kernel/src/main.rs`
  and `kernel/src/main_aarch64.rs`;
- the classes used today: timer, network receive and transmit, block, and the
  scheduler. Unused classes need not exist.

Nesting counters: `kernel/src/arch_impl/aarch64/constants.rs:191-231`, the x86
equivalents in `kernel/src/arch_impl/x86_64/constants.rs`, and
`kernel/src/per_cpu.rs:183-190`. The field widths are the rewriter's to choose,
from what Breenix needs: preemption nesting, interrupt nesting, deferred-work
execution and hold-off nesting, NMI.

**Requirements.**

1. A class raised from any context runs at least once after the raise, on the CPU
   that raised it, unless that CPU goes offline first. Work raised while a class
   is running runs again.
2. Run on interrupt exit only when the CPU is not already in hard IRQ, not already
   running deferred work, and not holding it off. Not from NMI.
3. A per-run bound, by iterations or time. Past it, the remainder is handed to that
   CPU's thread and the interrupt path returns.
4. Hold-off is per CPU and nests. Releasing the last hold-off runs anything
   pending.
5. The tier-1 constraints in CLAUDE.md apply to the raise and run paths: no
   logging, no locks without a try-lock fallback, no allocation.

**Governing material.** The tier-1 rules in this repo's CLAUDE.md. The Arm ARM
(DDI 0487) D1, "Exception levels and exception handling", and the Intel SDM Vol. 3A
ch. 6, "Interrupt and Exception Handling", for what interrupt-exit context may
do. Salim, Olsson and Kuznetsov, "Beyond Softnet" (2001), for budgeted receive
processing.

**Deterministic tests.** Host-model tests of requirements 1 to 4 over fixed
sequences:

- raise during run;
- raise while held off, then release;
- a storm exceeding the bound hands off exactly the remainder;
- a nested interrupt during a run does not re-enter.

Keep the guest-side cases in `kernel/src/task/softirq_tests.rs`, reworded.

## Part C: where each area's work happens

| Area | Verdict | Where | Notes |
| --- | --- | --- | --- |
| A1 waitqueue | SUBSTANTIVE | **E2** (sleepable contexts, park contract) | The E2 lane attaches B1. The rewriter does not read `task/waitqueue.rs` bodies, `scheduler.rs:6203-6262` or `6374-6420`, or the F32 docs |
| A9 legacy virtio-gpu PCI | SUBSTANTIVE, unreached | **Standalone PR** | A deletion, not a rewrite. B2 |
| A13 softirq | UNCLEAR | Coordinator. If SUBSTANTIVE: **E9** (per-CPU dispatch) or a standalone PR | B3 |
| A2, A3, A4, A5 | TRIVIAL | E2 / E9 rewrite them from specifications as a matter of course (DEV-35(2)) | No brief needed |
| A7 | TRIVIAL | E4 | |
| A8, A10 | TRIVIAL | E10 | |
| A6 | TRIVIAL | Done (e63a5ed6, 91d5f17e) | |
| A11, A12, A14, A15, A16, A17 | TRIVIAL | No action | |
| A18 | Out of scope | No action | |
| A19 | Fixed | #3246 | |

## Appendix: each file in the scan, by area

| Area | Files from the scan list |
| --- | --- |
| A1 | `docs/planning/f32-waitqueue/design.md`; `f32c-waitqueue/exit.md`, `ordering-audit.md`; `f32e-linux-prepare-to-wait/audit.md`, `exit.md`; `f32f-immediate-wake/audit.md`, `decisions.md`, `exit.md`, `scratchpad.md`; `kernel/src/task/waitqueue.rs`; `docs/planning/green-program/ipc/serials/813-c/{land2,reverify-960}/citation-audit.txt`, `813-c/source-citations.txt` (grep output that quotes our own waitqueue comment) |
| A2 | `kernel/src/task/completion.rs`; `docs/planning/f32h-ahci-wake/proposal.md` (completion part) |
| A3 | `docs/planning/f32i-cpu0-wfi-wake/linux-audit.md`, `linux-probe-validation.md`, `proposal.md`; `f32j-idle-sgi-admission/exit.md`, `prompt.md`; `f16-scheduler-audit.md`; `kernel/src/arch_impl/aarch64/context_switch.rs` (7313) |
| A4 | `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md` (1660-1666, 2797-3096); `f20e-audit-bisect/exit.md`; `kernel/src/arch_impl/aarch64/gic.rs`; `kernel/src/arch_impl/aarch64/timer_interrupt.rs`; `docs/polling-allowlist.md` (38, 59) |
| A5 | `kernel/src/arch_impl/aarch64/context_switch.rs` (7022); `kernel/src/task/scheduler.rs` (2118, 5893, 7058); `kernel/src/task/thread.rs` |
| A6 | `docs/planning/f32t-xhci-msi/linux-ground-truth.md` (MSI part); `docs/planning/green-program/sweeps/input-gui-aarch64-2026-09-06/EVIDENCE.md`, `evidence/source/482.txt`; `docs/polling-allowlist.md` (18) |
| A7 | `docs/planning/f32h-ahci-wake/audit.md`; `docs/artifacts/turn53-artifacts/p12-ahci-classification.md`; `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md` (3258); `docs/polling-allowlist.md` (91, 111); `kernel/src/drivers/ahci/mod.rs` |
| A8 | `kernel/src/net/tcp.rs`; `kernel/src/socket/tcp.rs` |
| A9 | `kernel/src/drivers/virtio/gpu_pci.rs`; the four `turn1-artifacts` copies added by fb9d81ef (removed by #3246) |
| A10 | `kernel/src/drivers/virtio/net_pci.rs`; `docs/polling-allowlist.md` (28) |
| A11 | `kernel/src/signal/{constants,delivery,types}.rs`; `kernel/src/syscall/signal.rs`; `kernel/src/interrupts.rs`; `docs/planning/teardown-unification/{DESIGN,PLAN}.md`; `docs/suites/README.md`; `userspace/programs/src/signal_test.rs` |
| A12 | `kernel/src/ipc/mqueue.rs`; `kernel/src/ipc/mqueue/store.rs`; `kernel/src/ipc/sysv/{msg,sem,shm,state}.rs` |
| A13 | `docs/planning/KTHREAD_IMPLEMENTATION_PLAN.md` (including the BSD lines 512-513) |
| A14 | `docs/planning/PTY_IMPLEMENTATION_PLAN.md` (including the BSD line 850); `kernel/src/tty/jobctl.rs` |
| A15 | `kernel/src/fs/procfs/pid_stat.rs` |
| A16 | `kernel/src/process/{manager,process}.rs`; `kernel/src/syscall/{wait,epoll,socket,fs,userptr}.rs`; `kernel/src/socket/{opts,types}.rs`; `userspace/libc-suite/abi_check.c`; `userspace/programs/src/{suite_memory,suite_time}.rs`; `tests/libc_sched_cpuset_host.py`; `tests/stdio-host/support.rs`; `docs/planning/x86-smp/X86-SMP-CENSUS-PLAN-2026-09-05.md` (names our own `ap_trampoline.S`) |
| A17 | `docs/gui/{e22-2-u9b,gpu-abi,gpu,gt710-c3a}.md`; `docs/gui/provenance/gt710-c5{a,c,d}-sources.json`; `kernel/assets/kepler/manifest.json`; `kernel/src/gpu/gk208/caps.rs`; `libs/gpu-abi/src/lib.rs`; `libs/gpu-abi/tests/gk208_contract.rs`; `tests/fixtures/gt710_shader_controls/provenance.json`; `tests/gt710_caps_test.rs` |
| A18 | `docs/gui/qemu-gl.md`; `docs/planning/arm64-catchup/{a3-epoxy-blob,a3-fix-design,a3-qemu-fork,a4-u7-ui-testing-baseline}.md` |
| A19 | `.agents/skills/breenix-interrupt-syscall-development/SKILL.md`; `.claude/commands/os-research.md`; `.claude/skills/os-research/SKILL.md` |
| Removed copies | `docs/artifacts/turn1-artifacts/linux-probe/*` and `linux-source/*` (six files, removed by #3246; `ehci-hcd.c` and `virtio_input.c` came from 361c5a6c and had no live consumer: A6, and `virtio/input_mmio.rs` predates them and holds evdev UAPI) |
