# ARM64 Syscall Porting Checklist

Goal: remove ARM64 ENOSYS stubs and wire shared AMD64 syscall implementations where possible.

This checklist maps each ARM64 stub to its AMD64 implementation module and the likely dependencies to port.

Legend: [ ] not started, [~] in progress, [x] done

## I/O and FD layer
- [ ] read -> `kernel/src/syscall/handlers.rs` (sys_read) + `kernel/src/ipc/fd/*`
  - Dependencies: fd table, VFS read path, file objects, pipe read
- [ ] write -> `kernel/src/syscall/handlers.rs` (sys_write) + `kernel/src/ipc/fd/*`
  - Dependencies: fd table, VFS write path, TTY write
- [ ] close -> `kernel/src/syscall/pipe.rs` (sys_close)
  - Dependencies: fd table close semantics
- [ ] dup/dup2 -> `kernel/src/syscall/handlers.rs` (sys_dup, sys_dup2)
  - Dependencies: fd table, refcounting
- [ ] fcntl -> `kernel/src/syscall/handlers.rs` (sys_fcntl)
  - Dependencies: fd flags, status flags
- [ ] poll/select -> `kernel/src/syscall/handlers.rs` (sys_poll, sys_select)
  - Dependencies: wait queues, eventing, fd readiness
- [ ] ioctl -> `kernel/src/syscall/ioctl.rs`
  - Dependencies: TTY/PTY ioctls, device ioctl routing
- [ ] pipe/pipe2 -> `kernel/src/syscall/pipe.rs`
  - Dependencies: pipe subsystem, fd table

## Filesystem
- [ ] open -> `kernel/src/syscall/fs.rs` (sys_open)
- [ ] read/write -> shared via fd table (see I/O)
- [ ] lseek -> `kernel/src/syscall/fs.rs` (sys_lseek)
- [ ] fstat -> `kernel/src/syscall/fs.rs` (sys_fstat)
- [ ] getdents64 -> `kernel/src/syscall/fs.rs` (sys_getdents64)
- [ ] access -> `kernel/src/syscall/fs.rs` (sys_access)
- [ ] getcwd -> `kernel/src/syscall/fs.rs` (sys_getcwd)
- [ ] chdir -> `kernel/src/syscall/fs.rs` (sys_chdir)
- [ ] rename -> `kernel/src/syscall/fs.rs` (sys_rename)
- [ ] mkdir -> `kernel/src/syscall/fs.rs` (sys_mkdir)
- [ ] rmdir -> `kernel/src/syscall/fs.rs` (sys_rmdir)
- [ ] link -> `kernel/src/syscall/fs.rs` (sys_link)
- [ ] unlink -> `kernel/src/syscall/fs.rs` (sys_unlink)
- [ ] symlink -> `kernel/src/syscall/fs.rs` (sys_symlink)
- [ ] readlink -> `kernel/src/syscall/fs.rs` (sys_readlink)
- [ ] mknod -> `kernel/src/syscall/fifo.rs` (sys_mknod)

## Sessions and Job Control
- [ ] setpgid -> `kernel/src/syscall/session.rs` (sys_setpgid)
- [ ] setsid -> `kernel/src/syscall/session.rs` (sys_setsid)
- [ ] getpgid -> `kernel/src/syscall/session.rs` (sys_getpgid)
- [ ] getsid -> `kernel/src/syscall/session.rs` (sys_getsid)

## PTY
- [ ] posix_openpt -> `kernel/src/syscall/pty.rs` (sys_posix_openpt)
- [ ] grantpt -> `kernel/src/syscall/pty.rs` (sys_grantpt)
- [ ] unlockpt -> `kernel/src/syscall/pty.rs` (sys_unlockpt)
- [ ] ptsname -> `kernel/src/syscall/pty.rs` (sys_ptsname)

## Process
- [ ] exec -> `kernel/src/syscall/handlers.rs` (sys_execv_with_frame)
  - ARM64 currently uses test loader in `sys_exec_aarch64`
  - Needs full filesystem-backed exec + ELF loader parity
- [ ] wait4 -> `kernel/src/syscall/handlers.rs` (sys_waitpid)

## Graphics
- [ ] fbinfo -> `kernel/src/syscall/graphics.rs` (sys_fbinfo)
- [ ] fbdraw -> `kernel/src/syscall/graphics.rs` (sys_fbdraw)

## Testing
- [ ] cow_stats -> `kernel/src/syscall/handlers.rs` (sys_cow_stats)
- [ ] simulate_oom -> `kernel/src/syscall/handlers.rs` (sys_simulate_oom)

---

# Notes on Architecture Dependencies

These modules are currently `#[cfg(target_arch = "x86_64")]` and must be made arch-neutral or given ARM64 implementations:

- `kernel/src/syscall/handlers.rs`
- `kernel/src/syscall/fs.rs`
- `kernel/src/syscall/ioctl.rs`
- `kernel/src/syscall/pipe.rs`
- `kernel/src/syscall/pty.rs`
- `kernel/src/syscall/session.rs`
- `kernel/src/syscall/graphics.rs`
- `kernel/src/syscall/fifo.rs`

Common blockers to resolve per module:
- user pointer validation (ARM64 user VA split)
- page table access / VMA checks
- per-CPU accessors (ARM64 per-CPU vs x86_64 GS)
- interrupt masking helpers (ARM64 CPU trait)
- timekeeping / timer reset semantics

---

# Immediate Execution Order (Recommended)
1) Enable `handlers.rs` and core fd operations on ARM64 (read/write/close/dup/pipe)
2) Enable basic filesystem syscalls (open/fstat/getdents/chdir/getcwd)
3) Enable session + PTY syscalls for init_shell job control
4) Replace ARM64 exec test loader with filesystem-backed exec
5) Enable graphics syscalls for userspace terminal

