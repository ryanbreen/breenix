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
