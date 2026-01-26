# ARM64 Feature Parity Plan (vs AMD64)

## Objective
Bring the ARM64 port to full feature parity with the current AMD64 implementation, including:
- Boot to a **userspace** shell (not kernel shell)
- Interactive TTY with job control
- Filesystem (ext2 + VFS + devfs/devpts)
- Working drivers (block, net, input, GPU)
- Working network stack (UDP + TCP + DNS/HTTP userspace tests)
- Passing the existing userspace test suite (or documented parity subset)

This plan is deliberately frank about gaps found in the current ARM64 code path.

## Current ARM64 State (Observed)

### What Works (early stage)
- AArch64 boot path exists and reaches kernel main.
- Basic MMU enable with identity mapping.
- GIC + timer bring-up with IRQ handling.
- VirtIO MMIO enumeration and basic device init.
- Kernel-mode graphics terminal + kernel shell loop.
- Minimal syscall entry/exit path for EL0.

### What Is Missing or Stubbed
- Userspace syscalls for FS/TTY/PTY/session/pipe/select/poll on ARM64.
- Userspace shell (init_shell) running from disk.
- File-based exec path for ARM64 (uses test disk loader only).
- Proper kernel heap allocator (ARM64 uses a bump allocator).
- User pointer validation uses x86_64 canonical split (unsafe on ARM64 identity map).
- Full scheduler/quantum reset and signal delivery on ARM64 return paths.
- TCP sockets on ARM64 are explicitly blocked.

## High-Risk Gaps (Blockers)
1. **User pointer validation is unsafe on ARM64**
   - `kernel/src/syscall/userptr.rs` uses x86_64 canonical split; kernel memory can be treated as user.
2. **ARM64 syscall coverage is incomplete**
   - Many syscalls return ENOSYS in `kernel/src/arch_impl/aarch64/syscall_entry.rs`.
3. **Kernel-mode shell is not parity**
   - Userspace init_shell depends on TTY/PTY/syscalls; current ARM64 uses `kernel/src/shell/mod.rs`.
4. **Memory subsystem parity not reached**
   - ARM64 boot uses hard-coded ranges and a bump allocator in `kernel/src/main_aarch64.rs`.

## AMD64 vs ARM64 Parity Matrix (Frank Status)

This section is deliberately blunt about what is missing on ARM64 compared to AMD64.

| Subsystem | AMD64 status (baseline) | ARM64 current state | Gap / risk | Required work |
| --- | --- | --- | --- | --- |
| Boot + MMU | High-half kernel + HHDM stable; CR3 behavior mature | High-half transition in progress; TTBR split booting but still evolving | Wrong mappings or identity-map assumptions break drivers | Finish high-half + HHDM mapping; remove identity-map assumptions |
| Memory map / discovery | Uses platform-provided memory map | ARM64 uses fixed ranges; no DTB memory map integration | Wrong RAM sizing, allocator bugs | Parse DTB memory map and feed allocator |
| Kernel heap | Tiered allocator / real heap | ARM64 uses bump allocator | Fragmentation, OOM under load | Enable full allocator on ARM64 |
| User pointers | Validated for x86_64 layout | ARM64 userptr was unsafe; now partially aligned with high-half | Security risk + EFAULT mismatch | Complete ARM64 userptr validation for new VA layout |
| Scheduler + preemption | Preemptive scheduling stable | ARM64 preemption not fully validated | Timing bugs, missed signals | Ensure timer IRQ drives scheduler; verify preemption on ARM64 |
| Signal delivery | AMD64 SA_ONSTACK + sigreturn working | ARM64 delivery path exists but not parity-verified | SA_ONSTACK, sigreturn, mask restore on ARM64 | Validate signal delivery on ARM64 and fix path divergences |
| Syscall coverage | Broad syscall set for tests/shell | Many ARM64 syscalls return ENOSYS (FS/TTY/PTY/session/pipe/poll/select/ioctl/exec/wait4) | Userspace shell cannot run | Remove ENOSYS stubs, wire to shared implementations |
| Exec / ELF | Exec from ext2 works; argv supported | ARM64 exec path incomplete | Cannot boot to userspace shell | Implement exec from ext2 for ARM64 |
| VFS/ext2 | VFS + ext2 stable | ARM64 syscalls stubbed; driver not fully exercised | No filesystem for userspace | Wire syscalls and verify ext2 on ARM64 |
| devfs / devpts | Working on AMD64 | Not wired on ARM64 | PTY + /dev missing | Enable devfs/devpts mounts on ARM64 |
| TTY + PTY | Full interactive shell + job control | ARM64 uses kernel shell; PTY syscalls stubbed | No interactive userspace | Implement PTY syscalls + line discipline for ARM64 |
| VirtIO block | AMD64 stable (PCI) | ARM64 MMIO driver in progress | Storage I/O unreliable | Confirm MMIO queues + IRQs + HHDM DMA |
| VirtIO net | AMD64 stable | ARM64 MMIO wired but TCP blocked | Networking incomplete | Enable TCP on ARM64; validate RX/TX path |
| VirtIO GPU/input | AMD64 stable | ARM64 MMIO in progress | No interactive UI | Confirm MMIO registers + input routing |
| IPC (pipes, sockets) | Pipes, UNIX sockets, UDP/TCP | ARM64 stubs for pipe/select/poll | Userspace blocked | Port IPC syscalls and polling |
| Userland shell | init_shell + coreutils on ext2 | Kernel shell only | Not parity | Build/install ARM64 userland and boot into init_shell |
| CI / tests | Boot stages + userspace tests | ARM64 manual workflow only | No parity signal in CI | Add ARM64 parity subsets once core syscalls work |

## Parity Scope (Definition of Done)
- Boot into EL0 init_shell from ext2 filesystem image.
- TTY input + canonical/raw modes + job control, signals, Ctrl-C.
- Coreutils run from disk (`/bin/ls`, `/bin/cat`, etc.).
- Network stack passes UDP + TCP userspace tests.
- No ARM64-only hacks in hot paths; interrupt/syscall timing constraints respected.

---

# Phased Plan

## Phase 0 - Baseline & Tracking (1-2 days)
- Create a parity checklist that maps AMD64 features to ARM64 status.
- Identify the exact test suite used to define parity.

Deliverables:
- Parity checklist in this doc (or linked file).
- Test list with pass/fail expectations.

## Phase 1 - Platform & Memory Foundations (High Priority)
- Replace hard-coded memory ranges with platform-provided memory map (UEFI/DTB).
- Integrate ARM64 page tables with `ProcessPageTable`/VMA/COW flows.
- Replace bump allocator with real kernel heap for ARM64.
- Fix userspace pointer validation for ARM64 user/kernel split.

Deliverables:
- ARM64 boot uses real memory map, not static ranges.
- Kernel heap allocator enabled on ARM64.
- Userspace pointer validation blocks kernel addresses.

Execution note (in progress):
- High-half kernel + TTBR0/TTBR1 split is now being implemented in `boot.S` + `linker.ld`.

Primary files:
- `kernel/src/main_aarch64.rs`
- `kernel/src/arch_impl/aarch64/mmu.rs`
- `kernel/src/memory/*`
- `kernel/src/syscall/userptr.rs`

## Phase 2 - Interrupts, Timer, and Preemption
- Remove serial/log output from ARM64 IRQ hot paths.
- Wire timer IRQ to scheduler quantum reset.
- Implement correct SP_EL0 handling in `Aarch64ExceptionFrame`.

Deliverables:
- Preemptive scheduling stable under load.
- IRQ paths are minimal and timing-safe.

Primary files:
- `kernel/src/arch_impl/aarch64/exception.rs`
- `kernel/src/arch_impl/aarch64/timer_interrupt.rs`
- `kernel/src/arch_impl/aarch64/exception_frame.rs`
- `kernel/src/arch_impl/aarch64/context_switch.rs`

## Phase 3 - Syscall Parity (Core)
- Remove ARM64 ENOSYS stubs for FS/TTY/PTY/session/pipe/select/poll.
- Wire shared syscall modules for ARM64 by loosening `cfg(target_arch)` gates.
- Validate ARM64 ABI struct layouts for stat/dirent/time/sigset.

Deliverables:
- ARM64 passes syscall tests that currently pass on AMD64.

Primary files:
- `kernel/src/syscall/mod.rs`
- `kernel/src/syscall/fs.rs`
- `kernel/src/syscall/pipe.rs`
- `kernel/src/syscall/pty.rs`
- `kernel/src/syscall/session.rs`
- `kernel/src/syscall/ioctl.rs`

## Phase 4 - Filesystem & Storage
- Ensure VirtIO MMIO block is interrupt-capable and stable.
- Enable devfs + devpts on ARM64 and mount at boot.
- Confirm ext2 + VFS work with ARM64 syscalls.

Deliverables:
- File open/read/write/getdents/fstat works on ARM64.
- /dev and /dev/pts are functional.

Primary files:
- `kernel/src/fs/*`
- `kernel/src/drivers/virtio/block_mmio.rs`
- `kernel/src/syscall/fs.rs`

## Phase 5 - TTY/PTY and Console
- Route VirtIO input to TTY line discipline.
- Implement PTY syscalls and `/dev/pts` for ARM64.
- Remove kernel shell dependency; use userspace init_shell.

Deliverables:
- init_shell runs in EL0 with real TTY semantics.
- Ctrl-C and job control functional.

Primary files:
- `kernel/src/tty/*`
- `kernel/src/syscall/pty.rs`
- `kernel/src/shell/mod.rs`

## Phase 6 - Userspace Exec & Init System
- Enable execve to load from filesystem (not test disk only).
- Build/install ARM64 userspace binaries on ext2 image.
- Ensure fork/exec/wait semantics are correct.

Deliverables:
- ARM64 boots to userspace init_shell from disk.
- Coreutils execute from `/bin`.

Primary files:
- `kernel/src/arch_impl/aarch64/elf.rs`
- `kernel/src/process/manager.rs`
- `userspace/examples/init_shell.rs`
- `xtask/src/ext2_disk.rs`

## Phase 7 - Network Parity
- Enable TCP support on ARM64 (remove AF_INET SOCK_STREAM limitation).
- Align socket syscall behavior with AMD64.
- Ensure net RX path works reliably (interrupt or polling as needed).

Deliverables:
- TCP/UDP userspace tests pass on ARM64.
- DNS/HTTP tests pass.

Primary files:
- `kernel/src/syscall/socket.rs`
- `kernel/src/net/*`
- `kernel/src/drivers/virtio/net_mmio.rs`

## Phase 8 - Driver & Graphics Parity
- Audit VirtIO MMIO drivers (block/net/gpu/input) for feature completeness.
- Ensure GPU/terminal integration is stable in userspace (not kernel shell).

Deliverables:
- Device enumeration + IRQ routing match AMD64 behavior.

Primary files:
- `kernel/src/drivers/virtio/*_mmio.rs`
- `kernel/src/graphics/*`

## Phase 9 - Test/CI Parity
- Add ARM64 QEMU smoke runs and parity test subsets.
- Ensure CI builds both arch targets and runs ARM64 tests where possible.

Deliverables:
- CI reports ARM64 parity status with no warnings.

Primary files:
- `tests/*`
- `userspace/tests/*`
- `scripts/run-arm64-qemu.sh`
- `xtask/src/main.rs`

---

# Risks and Notes
- Some parity work touches timing-sensitive paths; avoid logging in IRQ/syscall hot paths.
- ARM64 userptr validation is security-critical and should be fixed early.
- Until userspace init_shell runs on ARM64, the kernel shell is a temporary crutch.

# Next Steps (Recommended)
1. Fix ARM64 user pointer validation and memory map plumbing.
2. Wire syscall modules and remove ARM64 ENOSYS stubs for FS/TTY/PTY.
3. Boot into userspace init_shell from ext2 disk image.

---

# Parity Checklist (Living Document)
This checklist captures **what must match AMD64**. ARM64 status is intentionally blunt; any "unknown" item requires a concrete audit pass.

Legend: `[x]` parity verified, `[~]` partial/in-progress, `[ ]` missing/unknown

## Boot & Initialization
- [ ] UEFI/DTB memory map consumed and trusted (no static ranges)
- [ ] Per-CPU structures allocated and initialized
- [ ] SMP bring-up parity (APs start, enter scheduler)
- [ ] Userspace init process launched from filesystem image

## Memory & MMU
- [ ] VMA + COW flows usable by ARM64 page tables
- [ ] User/kernel address split enforced by userptr checks
- [ ] Kernel heap allocator active (no bump allocator)
- [ ] Fault handling parity (page faults, permissions, user faults)

## Scheduling, Signals, and Timers
- [ ] Preemptive scheduling with timer-based quantum reset
- [ ] Signal delivery path (incl. alt stack) matches AMD64
- [ ] sigreturn restores correct context on ARM64
- [ ] Timer IRQ handling is minimal and timing-safe

## Syscall Surface Parity
- [ ] FS syscalls (open/read/write/getdents/fstat/close/etc)
- [ ] TTY/PTY/session/setsid/ioctl
- [ ] pipe/dup/poll/select
- [ ] process (fork/exec/wait/exit/getpid)
- [ ] time (clock_gettime, nanosleep, etc)
- [ ] socket (UDP/TCP), bind/connect/accept/listen

## Filesystem & Storage
- [ ] ext2 read/write parity
- [ ] VFS + devfs + devpts mount parity
- [ ] VirtIO block MMIO: IRQ + queue features stable

## TTY/PTY & Shell
- [ ] VirtIO input routed to TTY line discipline
- [ ] /dev/pts functional (PTY pairs)
- [ ] Userspace init_shell runs with job control + signals

## Networking
- [ ] VirtIO net MMIO RX/TX stable
- [ ] UDP userspace tests pass
- [ ] TCP userspace tests pass (no ARM64 block)
- [ ] DNS/HTTP userspace tests pass

## Drivers & Graphics
- [ ] VirtIO GPU usable by userspace terminal
- [ ] VirtIO input/keyboard parity
- [ ] Any ARM64-specific device quirks documented

## CI/Test Parity
- [ ] ARM64 build is warning-free
- [ ] ARM64 test subset defined and tracked
- [ ] Boot stages (or equivalent) executed for ARM64

---

# Analysis Workstreams (Deep Diff Required)
This is the concrete work needed to **prove** AMD64 ↔ ARM64 parity and identify every gap.

## Workstream A - Syscall Matrix Diff
Goal: build an explicit list of syscalls that are implemented on AMD64 but ENOSYS or stubbed on ARM64.

Tasks:
- Inventory AMD64 syscall table and mapping (source of truth).
- Inventory ARM64 syscall entry mapping and `cfg(target_arch)` gates.
- Produce a per-syscall matrix with status: OK / stubbed / missing / ABI mismatch.
- Highlight syscalls required for init_shell + tests.

Deliverable:
- A table appended here or in a sibling doc: `ARM64_SYSCALL_MATRIX.md`.
- Current artifact: `docs/planning/ARM64_SYSCALL_MATRIX.md`.
- Porting checklist: `docs/planning/ARM64_SYSCALL_PORTING_CHECKLIST.md`.

## Workstream B - User/Kernel Memory Safety Audit
Goal: ensure ARM64 user memory validation and page table policy match AMD64 behavior.

Tasks:
- Audit `kernel/src/syscall/userptr.rs` and architecture-specific splits.
- Verify page fault handler parity (error codes, user vs kernel faults).
- Validate `ProcessPageTable` integration for ARM64 mappings.

Deliverable:
- Summary of differences and exact code locations; explicit fixes.
- Current artifact: `docs/planning/ARM64_USERPTR_AUDIT.md`.
- Memory layout diff: `docs/planning/ARM64_MEMORY_LAYOUT_DIFF.md`.

## Workstream C - Exec/ELF/Process Parity
Goal: ensure ARM64 exec path is real filesystem-backed, not test-only loader.

Tasks:
- Audit ARM64 ELF loader for correct auxv, stack layout, and permissions.
- Confirm execve path is shared and not gated for AMD64 only.
- Confirm fork/exec/wait semantics in scheduler and process manager.

Deliverable:
- A minimal boot-to-shell scenario documented with steps.

## Workstream D - Device & IRQ Path Parity
Goal: ensure VirtIO MMIO and IRQ routing is complete for block/net/input/gpu.

Tasks:
- Compare VirtIO MMIO feature negotiation and IRQ ack/EOI paths.
- Validate timer IRQ performance and preemption behavior.
- Confirm device drivers do not assume x86-specific features.

Deliverable:
- Driver parity checklist with explicit IRQ and feature gaps.

## Workstream E - Filesystem & TTY/PTY Parity
Goal: ensure init_shell has full TTY and filesystem semantics.

Tasks:
- Confirm devfs/devpts mount parity at boot.
- Validate PTY allocation and session leadership syscalls on ARM64.
- Ensure TTY line discipline receives VirtIO input.

Deliverable:
- A matrix of required shell syscalls and their ARM64 status.

---

# Milestones and Exit Criteria

## Milestone 1 - "Boot to Userspace"
Exit criteria:
- ARM64 boots to EL0 init_shell from ext2 image.
- Basic TTY input works (echo, backspace, newline).

## Milestone 2 - "Core Shell Workflow"
Exit criteria:
- `/bin/ls`, `/bin/cat` run from disk.
- Job control and Ctrl-C work.
- No kernel shell fallback in normal path.

## Milestone 3 - "Networking Online"
Exit criteria:
- UDP/TCP tests pass on ARM64.
- DNS/HTTP userspace tests pass.

## Milestone 4 - "Parity Lock"
Exit criteria:
- ARM64 passes the same userspace test suite as AMD64 (or documented, justified exceptions).
- No ARM64-only hacks in hot paths.

---

# Verification Strategy
- Use AMD64 tests as the gold standard; define the ARM64 subset explicitly and expand it to parity.
- Require warning-free ARM64 builds.
- Validate each subsystem with a minimal userspace test (filesystem, TTY, signals, networking).
