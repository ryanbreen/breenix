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
| Exec | OK | OK | ARM64 execv supports argv + ext2/test-disk fallback (testing feature only, same as AMD64). |
| Wait4 | OK | OK | ARM64 uses shared wait4/waitpid implementation. |
| GetPid | OK | OK | ARM64 returns real PID via scheduler/process manager. |
| GetTid | OK | OK | ARM64 returns real TID via scheduler. |
| Yield | OK | OK | ARM64 calls scheduler yield. |

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
| Read | OK | OK | ARM64 wired to shared fd/read path. |
| Write | OK | OK | ARM64 wired to shared fd/write path. |
| Close | OK | OK | ARM64 wired to shared fd/close path. |
| Pipe | OK | OK | ARM64 wired to shared pipe path. |
| Pipe2 | OK | OK | ARM64 wired to shared pipe2 path. |
| Dup | OK | OK | ARM64 wired to shared dup path. |
| Dup2 | OK | OK | ARM64 wired to shared dup2 path. |
| Fcntl | OK | OK | ARM64 wired to shared fcntl path. |
| Poll | OK | OK | ARM64 wired to shared poll path. |
| Select | OK | OK | ARM64 wired to shared select path. |
| Ioctl | OK | OK | ARM64 wired to shared ioctl path. |

## Filesystem
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| Access | OK | OK | ARM64 wired to shared VFS/ext2 path. |
| Getcwd | OK | OK | ARM64 wired to shared VFS path. |
| Chdir | OK | OK | ARM64 wired to shared VFS path. |
| Open | OK | OK | ARM64 wired to shared VFS path. |
| Lseek | OK | OK | ARM64 wired to shared VFS path. |
| Fstat | OK | OK | ARM64 wired to shared VFS path. |
| Getdents64 | OK | OK | ARM64 wired to shared VFS path. |
| Rename | OK | OK | ARM64 wired to shared VFS path. |
| Mkdir | OK | OK | ARM64 wired to shared VFS path. |
| Rmdir | OK | OK | ARM64 wired to shared VFS path. |
| Link | OK | OK | ARM64 wired to shared VFS path. |
| Unlink | OK | OK | ARM64 wired to shared VFS path. |
| Symlink | OK | OK | ARM64 wired to shared VFS path. |
| Readlink | OK | OK | ARM64 wired to shared VFS path. |
| Mknod | OK | OK | ARM64 wired to shared FIFO/mknod path. |

## Session / Job Control
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| SetPgid | OK | OK | ARM64 wired to shared session path. |
| SetSid | OK | OK | ARM64 wired to shared session path. |
| GetPgid | OK | OK | ARM64 wired to shared session path. |
| GetSid | OK | OK | ARM64 wired to shared session path. |

## PTY
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| PosixOpenpt | OK | PARTIAL | Syscalls wired; devptsfs now initialized on ARM64 (unverified). |
| Grantpt | OK | PARTIAL | Syscalls wired; devptsfs now initialized on ARM64 (unverified). |
| Unlockpt | OK | PARTIAL | Syscalls wired; devptsfs now initialized on ARM64 (unverified). |
| Ptsname | OK | PARTIAL | Syscalls wired; devptsfs now initialized on ARM64 (unverified). |

## Networking
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| Socket | OK | OK | TCP enabled on ARM64; UDP/Unix already supported (unverified). |
| Connect | OK | OK | TCP enabled on ARM64 (unverified). |
| Accept | OK | OK | TCP enabled on ARM64 (unverified). |
| SendTo | OK | OK | UDP/Unix/TCP paths wired (unverified). |
| RecvFrom | OK | OK | UDP/Unix/TCP paths wired (unverified). |
| Bind | OK | OK | UDP/Unix/TCP paths wired (unverified). |
| Listen | OK | OK | TCP enabled on ARM64 (unverified). |
| Shutdown | OK | OK | TCP enabled on ARM64 (unverified). |
| Socketpair | OK | OK | Unix domain only (by design). |

## Graphics / Testing
| Syscall | AMD64 | ARM64 | Notes |
|---|---|---|---|
| FbInfo | OK | OK | ARM64 wired to shared graphics syscalls. |
| FbDraw | OK | OK | ARM64 wired to shared graphics syscalls. |
| CowStats | OK | STUB | ARM64 returns ENOSYS. |
| SimulateOom | OK | STUB | ARM64 returns ENOSYS. |

---

# Immediate Parity Gaps (Blocking init_shell)
1. ARM64 userspace binaries installed on ext2 image (builder exists, coreutils coverage TBD)
2. PTY/TTY validation under ARM64 userspace load
3. Scheduler/preemption validation under userspace load
4. Memory map + allocator parity (still bump allocator)
5. ARM64 test harness / boot stage parity subset

# Next Actionable Step
Update plan docs after each major parity milestone and prioritize devptsfs + allocator parity work next.
