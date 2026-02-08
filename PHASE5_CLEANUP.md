# Phase 5: Cleanup — Remove no_std Duplicates, Make std the Only Userspace

## Goal

Every userspace program should be built with real Rust `std`. Remove all no_std
duplicates where std versions exist. Port the remaining no_std-only programs.
Update the build system so `kernel/build.rs` invokes the std build path.

## Current State

- **71 std programs** in `userspace/tests-std/` (built with `-Z build-std=std,panic_abort`)
- **117 no_std programs** in `userspace/tests/` (built with libbreenix, `#![no_std]`)
- **15 no_std coreutils** in `userspace/bin/coreutils/`
- **6 no_std examples** in `userspace/examples/`
- **2 no_std services** in `userspace/bin/services/` (init, telnetd)

The std build (`tests-std/build.sh`) copies its output as `.elf` files into
`userspace/tests/` (or `userspace/tests/aarch64/`), overwriting the no_std
versions with the same name. So 71 programs already run as std on the test disk.

## Build System Architecture

```
kernel/build.rs
  └─ calls: userspace/tests/build.sh     (no_std — compiles 117 binaries)
  └─ does NOT call: userspace/tests-std/build.sh  (std — must be run separately)

userspace/tests-std/build.sh
  └─ builds 71 std binaries
  └─ copies them to userspace/tests/*.elf (overwrites no_std versions)

Test disk (BXTEST format on VirtIO device 1):
  └─ created from all *.elf files in userspace/tests/
  └─ kernel loads binaries via get_test_binary(name) in userspace_test.rs

EXT2 disk (VirtIO device 0):
  └─ contains /sbin/init, /sbin/telnetd, /bin/init_shell, coreutils in /bin/
  └─ created by docker/qemu/create_ext2_disk.sh
```

## Task 1: Port Remaining no_std-Only Programs to std

These 46 programs exist only as no_std. Each needs a std port in
`userspace/tests-std/src/` with a `[[bin]]` entry in
`userspace/tests-std/Cargo.toml` and an entry in `build.sh`'s `STD_BINARIES`.

### Tests to port (31)

| Program | Category | Notes |
|---------|----------|-------|
| timer_test | timing | |
| alarm_test | signals | uses alarm() syscall |
| itimer_test | signals | uses setitimer() |
| signal_test | signals | basic signal test |
| sleep_debug_test | timing | |
| register_init_test | syscall | checks initial register state |
| syscall_diagnostic_test | syscall | |
| test_mmap | memory | mmap test |
| argv_test | process | argc/argv validation |
| exec_stack_argv_test | process | |
| stdin_test | io | reads from stdin |
| pipe_fork_test | ipc | pipe + fork |
| pipe_concurrent_test | ipc | |
| pipe_refcount_test | ipc | |
| job_table_test | process | |
| cow_oom_test | memory | CoW out-of-memory |
| true_test | coreutils | tests /bin/true |
| false_test | coreutils | tests /bin/false |
| head_test | coreutils | tests /bin/head |
| tail_test | coreutils | tests /bin/tail |
| wc_test | coreutils | tests /bin/wc |
| which_test | coreutils | tests /bin/which |
| cat_test | coreutils | tests /bin/cat |
| ls_test | coreutils | tests /bin/ls |
| mkdir_argv_test | coreutils | tests mkdir with args |
| cp_mv_argv_test | coreutils | tests cp/mv with args |
| echo_argv_test | coreutils | tests echo with args |
| rm_argv_test | coreutils | tests rm with args |
| fs_block_alloc_test | filesystem | |
| exec_from_ext2_test | filesystem | exec from ext2 |
| pty_test | tty | pseudo-terminal |

### Network tests to port (8)

| Program | Notes |
|---------|-------|
| udp_socket_test | UDP sockets |
| tcp_socket_test | TCP sockets |
| tcp_blocking_test | blocking TCP |
| tcp_client_test | TCP client |
| blocking_recv_test | blocking recv |
| concurrent_recv_stress | stress test |
| nonblock_eagain_test | EAGAIN handling |
| dns_test | DNS resolution |

### Interactive/demo programs (5) — keep as no_std or skip

| Program | Notes |
|---------|-------|
| demo | graphics demo — not a test, skip |
| bounce | animation demo — not a test, skip |
| particles | particle demo — not a test, skip |
| fbinfo_test | framebuffer info — port if used in tests |
| http_test | HTTP client — port if used in tests |

### Examples (5) — keep as no_std or remove

| Program | Notes |
|---------|-------|
| hello_time | replaced by clock_gettime_test std |
| hello_world | replaced by hello_std_real |
| simple_exit | trivial, not needed |
| counter | demo, not a test |
| spinner | demo, not a test |

### Services (2) — port to std

| Program | Notes |
|---------|-------|
| init | /sbin/init — the PID 1 process |
| telnetd | /sbin/telnetd — telnet server |

**These are critical.** The init_shell already has a std port. init and telnetd
need std ports as well. They go on the EXT2 disk, not the test disk.

## Task 2: Remove no_std Duplicates

After std ports are verified, remove the 71 no_std source files that have
std replacements. These are all in `userspace/tests/`:

```
access_test.rs        clock_gettime_test.rs  cloexec_test.rs
cow_cleanup_test.rs   cow_readonly_test.rs   cow_signal_test.rs
cow_sole_owner_test.rs cow_stress_test.rs    ctrl_c_test.rs
cwd_test.rs           devfs_test.rs          dup_test.rs
exec_argv_test.rs     fcntl_test.rs          fifo_test.rs
file_read_test.rs     fork_memory_test.rs    fork_pending_signal_test.rs
fork_state_test.rs    fork_test.rs           fs_directory_test.rs
fs_large_file_test.rs fs_link_test.rs        fs_rename_test.rs
fs_write_test.rs      getdents_test.rs       job_control_test.rs
kill_process_group_test.rs  lseek_test.rs    nonblock_test.rs
pause_test.rs         pipe_test.rs           pipe2_test.rs
pipeline_test.rs      poll_test.rs           select_test.rs
session_test.rs       shell_pipe_test.rs     sigaltstack_test.rs
sigchld_job_test.rs   sigchld_test.rs        signal_exec_test.rs
signal_fork_test.rs   signal_handler_test.rs signal_regs_test.rs
signal_return_test.rs sigsuspend_test.rs     syscall_enosys.rs
tty_test.rs           unix_named_socket_test.rs  unix_socket_test.rs
waitpid_test.rs       wnohang_timing_test.rs brk_test.rs
```

Also remove the no_std versions of coreutils and init_shell whose std versions
exist (the source files in `userspace/bin/coreutils/` and
`userspace/examples/init_shell.rs`):

```
userspace/bin/coreutils/cat.rs    userspace/bin/coreutils/cp.rs
userspace/bin/coreutils/echo.rs   userspace/bin/coreutils/false.rs
userspace/bin/coreutils/head.rs   userspace/bin/coreutils/ls.rs
userspace/bin/coreutils/mkdir.rs  userspace/bin/coreutils/mv.rs
userspace/bin/coreutils/rm.rs     userspace/bin/coreutils/rmdir.rs
userspace/bin/coreutils/tail.rs   userspace/bin/coreutils/true.rs
userspace/bin/coreutils/wc.rs     userspace/bin/coreutils/which.rs
userspace/bin/coreutils/resolution.rs
userspace/examples/init_shell.rs
```

## Task 3: Update `userspace/tests/Cargo.toml`

Remove all `[[bin]]` entries for programs that now only exist as std. Keep
entries for:
- Programs not yet ported to std (the 46 listed in Task 1)
- Services (init, telnetd) until they're ported
- signal_exec_check (helper binary used by signal_exec_test)

## Task 4: Update `kernel/build.rs`

Change `kernel/build.rs` to ALSO call `userspace/tests-std/build.sh` after
the no_std build. Or better: make `tests/build.sh` invoke `tests-std/build.sh`
at the end so the std binaries overwrite the no_std ones automatically.

## Task 5: Update EXT2 Disk Creation

`docker/qemu/create_ext2_disk.sh` (or equivalent) needs to use the std-built
coreutils and init_shell instead of the no_std versions. Check how it currently
finds the binaries and update paths.

## Task 6: Verify

After cleanup:

```bash
# Build ARM64
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64

# Run ARM64 boot test
./docker/qemu/run-aarch64-boot-test-native.sh

# Build x86_64
cargo build --release --features testing,external_test_bins --bin qemu-uefi

# Run x86_64 boot tests
./docker/qemu/run-boot-parallel.sh 1
```

Check serial output for:
- All test markers still present (PASSED, etc.)
- No missing binary errors
- init_shell boots to prompt
- telnetd starts and listens

## Porting Pattern

Each no_std program follows a consistent pattern. To port to std:

```rust
// BEFORE (no_std)
#![no_std]
#![no_main]
use libbreenix::io::println;
use libbreenix::process::exit;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("hello");
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { exit(1); }

// AFTER (std)
fn main() {
    println!("hello");
}
```

For syscalls not in std (fork, signals, raw fd ops), use `unsafe extern "C"`:

```rust
extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
}
```

These resolve to libbreenix-libc functions at link time.

**Critical**: Every ported program MUST emit the exact same test markers
(e.g., `TIMER_TEST_PASSED`) that the kernel test infrastructure checks for.

## Dependency Order

```
Task 1 (port remaining) ──► Task 2 (remove duplicates)
                          ──► Task 3 (update Cargo.toml)
                          ──► Task 4 (update build.rs)
                          ──► Task 5 (update ext2 disk)
                                      │
                                      ▼
                               Task 6 (verify)
```

Tasks 2-5 can be done in parallel after Task 1 completes for each batch.
