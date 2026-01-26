# ARM64 Syscall Parity Matrix (vs AMD64)

This matrix is derived from:
- AMD64 dispatcher: `kernel/src/syscall/handler.rs`
- ARM64 dispatcher: `kernel/src/arch_impl/aarch64/syscall_entry.rs`
- Syscall list: `kernel/src/syscall/mod.rs`

Legend: **OK** = implemented and used on AMD64, **PARTIAL** = implemented but not parity, **STUB** = returns ENOSYS or fake success, **FIXME** = correctness concerns

## Core Process / Scheduling
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| Exit | OK | PARTIAL | ARM64 prints + halts in WFI loop instead of terminating process. |
| Fork | OK | PARTIAL | ARM64 has `sys_fork_aarch64`, but scheduler/resched is still TODO in syscall return path. |
| Exec | OK | PARTIAL | ARM64 uses `sys_exec_aarch64` but is test-only (loads named test program, not full FS exec path). |
| Wait4 | OK | STUB | ARM64 returns ENOSYS. |
| GetPid | OK | STUB | ARM64 returns fixed `1`. |
| GetTid | OK | STUB | ARM64 returns fixed `1`. |
| Yield | OK | PARTIAL | ARM64 returns 0 (no scheduling effect). |

## Memory
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| Brk | OK | OK | Uses shared `sys_brk`. |
| Mmap | OK | OK | Uses shared `sys_mmap`. |
| Munmap | OK | OK | Uses shared `sys_munmap`. |
| Mprotect | OK | OK | Uses shared `sys_mprotect`. |

## Time
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| GetTime | OK | OK | ARM64 returns monotonic ns. |
| ClockGetTime | OK | OK | ARM64 uses local `sys_clock_gettime` (writes to user ptr directly). |

## Signals
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| Kill | OK | OK | Shared implementation. |
| Sigaction | OK | OK | Shared implementation. |
| Sigprocmask | OK | OK | Shared implementation. |
| Sigpending | OK | OK | Shared implementation. |
| Sigaltstack | OK | OK | Shared implementation. |
| Sigreturn | OK | OK | ARM64 has frame-aware path. |
| Pause | OK | OK | ARM64 has frame-aware path. |
| Sigsuspend | OK | OK | ARM64 has frame-aware path. |
| Alarm | OK | OK | Shared implementation. |
| Getitimer | OK | OK | Shared implementation. |
| Setitimer | OK | OK | Shared implementation. |

## I/O, Pipes, Polling, and FDs
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| Read | OK | STUB | ARM64 returns ENOSYS. |
| Write | OK | PARTIAL | ARM64 writes raw bytes to serial; no fd handling beyond stdout/stderr. |
| Close | OK | STUB | ARM64 returns 0 without closing. |
| Pipe | OK | STUB | ARM64 returns ENOSYS. |
| Pipe2 | OK | STUB | ARM64 returns ENOSYS. |
| Dup | OK | STUB | ARM64 returns ENOSYS. |
| Dup2 | OK | STUB | ARM64 returns ENOSYS. |
| Fcntl | OK | STUB | ARM64 returns ENOSYS. |
| Poll | OK | STUB | ARM64 returns ENOSYS. |
| Select | OK | STUB | ARM64 returns ENOSYS. |
| Ioctl | OK | STUB | ARM64 returns ENOSYS. |

## Filesystem
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| Access | OK | STUB | ARM64 returns ENOSYS. |
| Getcwd | OK | STUB | ARM64 returns ENOSYS. |
| Chdir | OK | STUB | ARM64 returns ENOSYS. |
| Open | OK | STUB | ARM64 returns ENOSYS. |
| Lseek | OK | STUB | ARM64 returns ENOSYS. |
| Fstat | OK | STUB | ARM64 returns ENOSYS. |
| Getdents64 | OK | STUB | ARM64 returns ENOSYS. |
| Rename | OK | STUB | ARM64 returns ENOSYS. |
| Mkdir | OK | STUB | ARM64 returns ENOSYS. |
| Rmdir | OK | STUB | ARM64 returns ENOSYS. |
| Link | OK | STUB | ARM64 returns ENOSYS. |
| Unlink | OK | STUB | ARM64 returns ENOSYS. |
| Symlink | OK | STUB | ARM64 returns ENOSYS. |
| Readlink | OK | STUB | ARM64 returns ENOSYS. |
| Mknod | OK | STUB | ARM64 returns ENOSYS. |

## Session / Job Control
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| SetPgid | OK | STUB | ARM64 returns ENOSYS. |
| SetSid | OK | STUB | ARM64 returns ENOSYS. |
| GetPgid | OK | STUB | ARM64 returns ENOSYS. |
| GetSid | OK | STUB | ARM64 returns ENOSYS. |

## PTY
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| PosixOpenpt | OK | STUB | ARM64 returns ENOSYS. |
| Grantpt | OK | STUB | ARM64 returns ENOSYS. |
| Unlockpt | OK | STUB | ARM64 returns ENOSYS. |
| Ptsname | OK | STUB | ARM64 returns ENOSYS. |

## Networking
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| Socket | OK | PARTIAL | ARM64 supports UDP + Unix; TCP (AF_INET/SOCK_STREAM) returns EAFNOSUPPORT. |
| Connect | OK | PARTIAL | Works for UDP/Unix; TCP blocked. |
| Accept | OK | PARTIAL | Works for Unix; TCP blocked. |
| SendTo | OK | PARTIAL | Works for UDP/Unix. |
| RecvFrom | OK | PARTIAL | Works for UDP/Unix. |
| Bind | OK | PARTIAL | Works for UDP/Unix. |
| Listen | OK | PARTIAL | Works for Unix; TCP blocked. |
| Shutdown | OK | PARTIAL | Works where sockets exist; TCP blocked. |
| Socketpair | OK | PARTIAL | Unix domain only. |

## Graphics / Testing
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| FbInfo | OK | STUB | ARM64 returns ENOSYS. |
| FbDraw | OK | STUB | ARM64 returns ENOSYS. |
| CowStats | OK | STUB | ARM64 returns ENOSYS. |
| SimulateOom | OK | STUB | ARM64 returns ENOSYS. |

---

# Immediate Parity Gaps (Blocking init_shell)
1. FS syscalls: open/read/write/close/getdents/fstat/chdir/getcwd
2. PTY + session syscalls for job control
3. Pipe/select/poll + basic FD semantics
4. Exec path from filesystem (not test loader)
5. Read/write implementations that use real file descriptors, not serial-only

# Next Actionable Step
Produce a per-syscall porting checklist that maps each ARM64 stub to the AMD64 implementation module and identifies any architecture-specific dependencies.
