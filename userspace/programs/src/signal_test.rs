//! Signal test program (std version)
//!
//! Tests basic signal functionality:
//! 1. kill() syscall to send SIGTERM to child
//! 2. Default signal handler (terminate)

#[cfg(target_arch = "x86_64")]
use instruction::{fork, waitpid};
#[cfg(not(target_arch = "x86_64"))]
use libbreenix::process::{fork, waitpid};
use libbreenix::process::{getpid, wtermsig, yield_now, ForkResult};

#[cfg(target_arch = "x86_64")]
mod instruction {
    use libbreenix::signal::{Sigaction, SA_RESTART, SA_RESTORER, SIGILL, SIGUSR1};
    use libbreenix::{error::Error, process::ForkResult, types::Pid};
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    // Explicit SYSCALL wrappers keep the existing INT 0x80 tests intact.
    pub unsafe fn call(nr: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
        let result: i64;
        core::arch::asm!("syscall", inlateout("rax") nr => result,
            in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d,
            lateout("rcx") _, lateout("r11") _, options(nostack));
        result
    }

    pub fn fork() -> Result<ForkResult, Error> {
        let pid = Error::from_syscall(unsafe { call(57, 0, 0, 0, 0) })?;
        Ok(if pid == 0 {
            ForkResult::Child
        } else {
            ForkResult::Parent(Pid::from_raw(pid))
        })
    }

    pub fn waitpid(pid: i32, status: *mut i32, options: i32) -> Result<Pid, Error> {
        Error::from_syscall(unsafe { call(61, pid as u64, status as u64, options as u64, 0) })
            .map(Pid::from_raw)
    }

    #[unsafe(naked)]
    extern "C" fn restore() -> ! {
        core::arch::naked_asm!("mov rax, 15", "syscall", "ud2");
    }

    fn action(sig: i32, handler: u64, flags: u64) {
        let act = Sigaction {
            handler,
            flags: flags | SA_RESTORER,
            restorer: restore as u64,
            mask: 0,
        };
        assert_eq!(
            unsafe { call(13, sig as u64, &act as *const _ as u64, 0, 8) },
            0
        );
    }

    static RETURNED: AtomicBool = AtomicBool::new(false);
    static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
    extern "C" fn returning_handler(sig: i32) {
        assert_eq!(sig, SIGUSR1);
        RETURNED.store(true, Ordering::SeqCst);
    }
    extern "C" fn caught_ud(sig: i32) {
        unsafe {
            call(60, if sig == SIGILL { 44 } else { 99 }, 0, 0, 0);
        }
        unreachable!();
    }
    static RESTARTED: AtomicBool = AtomicBool::new(false);

    // Capture the signal frame pointer before a Rust prologue changes RSP.
    #[unsafe(naked)]
    extern "C" fn restart_handler(_sig: i32) {
        core::arch::naked_asm!(
            "mov rsi, rsp",
            "push rbp",
            "mov rbp, rsp",
            "and rsp, -16",
            "call {body}",
            "mov rsp, rbp",
            "pop rbp",
            "ret",
            body = sym restart_handler_body,
        );
    }

    extern "C" fn restart_handler_body(sig: i32, frame: *const u64) {
        assert_eq!(sig, SIGUSR1);
        // The frame is Linux's rt_sigframe: the return address, then the
        // ucontext, whose uc_mcontext (at byte 48) saves RAX, RCX and RIP at
        // its bytes 104, 112 and 128.
        // ERESTARTSYS must restore READ's number and rewind onto SYSCALL.
        // RCX holds the next RIP only if SYSCALL ran: read_entered() loads
        // zero, so a frame interrupted before READ entered cannot match.
        let rip = unsafe { *frame.add(22) };
        let rax = unsafe { *frame.add(19) };
        let rcx = unsafe { *frame.add(20) };
        if rax == 0
            && rcx == rip.wrapping_add(2)
            && unsafe { std::ptr::read_unaligned(rip as *const u16) } == 0x050f
        {
            RESTARTED.store(true, Ordering::SeqCst);
            let byte = b'R';
            assert_eq!(
                unsafe {
                    call(
                        1,
                        WRITE_FD.load(Ordering::SeqCst) as u64,
                        &byte as *const _ as u64,
                        1,
                        0,
                    )
                },
                1
            );
        }
    }

    fn read_entered(fd: i32, byte: &mut u8) -> i64 {
        let result: i64;
        unsafe {
            core::arch::asm!("syscall", inlateout("rax") 0i64 => result,
                in("rdi") fd as u64, in("rsi") byte as *mut u8 as u64, in("rdx") 1u64,
                inlateout("rcx") 0u64 => _, lateout("r11") _, options(nostack));
        }
        result
    }

    fn blocked(status_path: &[u8]) -> bool {
        let fd = unsafe { call(2, status_path.as_ptr() as u64, 0, 0, 0) };
        if fd < 0 {
            return false;
        }
        let mut buf = [0u8; 512];
        let len = unsafe { call(0, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0) };
        unsafe { call(3, fd as u64, 0, 0, 0) };
        const STATE: &[u8] = b"State:\tBlocked";
        len > 0
            && buf[..len as usize]
                .windows(STATE.len())
                .any(|line| line == STATE)
    }

    pub fn check() {
        action(SIGUSR1, returning_handler as u64, 0);
        let pid = unsafe { call(39, 0, 0, 0, 0) };
        assert!(pid > 0);
        assert_eq!(unsafe { call(62, pid as u64, SIGUSR1 as u64, 0, 0) }, 0);
        assert!(RETURNED.load(Ordering::SeqCst));
        println!("USER_SYSCALL_SIGRETURN_PASSED");

        for disposition in 0..3 {
            match fork().expect("fork SIGILL disposition child") {
                ForkResult::Child => {
                    action(
                        SIGILL,
                        if disposition == 2 {
                            1
                        } else {
                            caught_ud as u64
                        },
                        0,
                    );
                    if disposition == 1 {
                        let mask = 1u64 << (SIGILL - 1);
                        assert_eq!(unsafe { call(14, 0, &mask as *const _ as u64, 0, 8) }, 0);
                    }
                    unsafe {
                        core::arch::asm!("ud2", options(noreturn));
                    }
                }
                ForkResult::Parent(child) => {
                    let mut status = 0;
                    assert_eq!(
                        waitpid(child.raw() as i32, &mut status, 0).unwrap().raw(),
                        child.raw()
                    );
                    if disposition == 0 {
                        assert!(libbreenix::process::wifexited(status));
                        assert_eq!(libbreenix::process::wexitstatus(status), 44);
                    } else {
                        assert_eq!(libbreenix::process::wtermsig(status), SIGILL);
                    }
                }
            }
        }
        println!("USER_UD_DISPOSITIONS_PASSED");

        let mut fds = [-1i32; 2];
        assert_eq!(unsafe { call(22, fds.as_mut_ptr() as u64, 0, 0, 0) }, 0);
        WRITE_FD.store(fds[1], Ordering::SeqCst);
        action(SIGUSR1, restart_handler as u64, SA_RESTART);
        let sender = match fork().expect("fork restart signal sender") {
            ForkResult::Child => {
                // Signal only while the reader sleeps, so the interrupted
                // READ is one that blocked and had to be woken.
                let status_path = format!("/proc/{pid}/status\0");
                loop {
                    if blocked(status_path.as_bytes()) {
                        assert_eq!(unsafe { call(62, pid as u64, SIGUSR1 as u64, 0, 0) }, 0);
                    }
                    assert_eq!(unsafe { call(24, 0, 0, 0, 0) }, 0);
                }
            }
            ForkResult::Parent(child) => child,
        };
        let mut byte = 0u8;
        // Signals received before read cannot fill the pipe. Only a handler
        // observing the rewound frame of an entered, blocked read writes, so
        // completion proves the wake, SA_RESTART and SYSCALL sigreturn resumed
        // the interrupted operation.
        assert_eq!(read_entered(fds[0], &mut byte), 1);
        assert_eq!(byte, b'R');
        assert!(RESTARTED.load(Ordering::SeqCst));
        assert_eq!(
            unsafe { call(62, sender.raw(), libbreenix::signal::SIGTERM as u64, 0, 0) },
            0
        );
        let mut status = 0;
        assert_eq!(
            waitpid(sender.raw() as i32, &mut status, 0).unwrap().raw(),
            sender.raw()
        );
        assert_eq!(
            libbreenix::process::wtermsig(status),
            libbreenix::signal::SIGTERM
        );
        for fd in fds {
            assert_eq!(unsafe { call(3, fd as u64, 0, 0, 0) }, 0);
        }
        println!("USER_SYSCALL_RESTART_PASSED");
    }
}
use libbreenix::signal::{kill, SIGTERM};

// Execute a SYSCALL ending at the last canonical user byte. Its hardware
// return RCX is non-canonical; SYSRET must be rejected and IRETQ's #GP must
// kill this child with SIGSEGV while the parent and the boot keep running.
#[cfg(target_arch = "x86_64")]
fn check_noncanonical_syscall_return() {
    use std::os::unix::fs::PermissionsExt;
    let mut elf = [0u8; 4096];
    elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    elf[16..18].copy_from_slice(&2u16.to_le_bytes());
    elf[18..20].copy_from_slice(&62u16.to_le_bytes());
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    elf[24..32].copy_from_slice(&0x7fff_ffff_fff9u64.to_le_bytes());
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());
    elf[52..54].copy_from_slice(&64u16.to_le_bytes());
    elf[54..56].copy_from_slice(&56u16.to_le_bytes());
    elf[56..58].copy_from_slice(&2u16.to_le_bytes());
    elf[64..68].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    elf[68..72].copy_from_slice(&5u32.to_le_bytes()); // readable/executable
    elf[80..88].copy_from_slice(&0x7fff_ffff_f000u64.to_le_bytes());
    elf[96..104].copy_from_slice(&4096u64.to_le_bytes());
    elf[104..112].copy_from_slice(&4096u64.to_le_bytes());
    elf[112..120].copy_from_slice(&4096u64.to_le_bytes());
    // An empty PT_LOAD is legal and must not underflow the segment end.
    elf[120..124].copy_from_slice(&1u32.to_le_bytes());
    elf[136..144].copy_from_slice(&0x4000_0000u64.to_le_bytes());
    elf[168..176].copy_from_slice(&4096u64.to_le_bytes());
    elf[4089..].copy_from_slice(&[0xb8, 39, 0, 0, 0, 0x0f, 0x05]); // getpid; syscall
    let path = "/tmp/syscall_edge.elf";
    std::fs::write(path, elf).expect("write syscall edge executable");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod syscall edge executable");
    match fork().expect("fork syscall edge child") {
        ForkResult::Child => {
            let argv = [b"syscall_edge\0".as_ptr(), std::ptr::null()];
            let error = libbreenix::process::execv(b"/tmp/syscall_edge.elf\0", argv.as_ptr());
            panic!("exec syscall edge executable failed: {:?}", error);
        }
        ForkResult::Parent(child) => {
            let mut status = 0;
            let reaped =
                waitpid(child.raw() as i32, &mut status, 0).expect("wait syscall edge child");
            assert_eq!(reaped.raw(), child.raw());
            assert_eq!(wtermsig(status), libbreenix::signal::SIGSEGV);
            std::fs::remove_file(path).expect("remove syscall edge executable");
            println!("USER_NONCANONICAL_SYSCALL_PASSED");
        }
    }
}

fn main() {
    println!("=== Signal Test ===");

    #[cfg(target_arch = "x86_64")]
    match fork().expect("fork #UD child") {
        ForkResult::Child => unsafe {
            core::arch::asm!("ud2", options(noreturn));
        },
        ForkResult::Parent(child) => {
            let mut status = 0;
            let reaped = waitpid(child.raw() as i32, &mut status, 0).expect("wait #UD child");
            assert_eq!(reaped.raw(), child.raw());
            assert_eq!(wtermsig(status), libbreenix::signal::SIGILL);
            println!("USER_UD_SIGILL_PASSED");
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        check_noncanonical_syscall_return();
        instruction::check();
    }

    let my_pid = getpid().unwrap().raw() as i32;
    println!("My PID: {}", my_pid);

    // Test 1: Check if process exists using kill(pid, 0)
    println!("\nTest 1: Check process exists with kill(pid, 0)");
    let ret = kill(my_pid, 0);
    if ret.is_ok() {
        println!("  PASS: Process exists");
    } else {
        println!("  FAIL: kill returned error");
    }

    // Test 2: Fork and send SIGTERM to child
    println!("\nTest 2: Fork and send SIGTERM to child");

    match fork() {
        Ok(ForkResult::Child) => {
            // Child process - loop forever, waiting for signal
            println!("  CHILD: Started, waiting for signal...");
            let child_pid = getpid().unwrap().raw() as i32;
            println!("  CHILD: My PID is {}", child_pid);

            // Busy loop - should be killed by parent
            let mut counter = 0u64;
            loop {
                counter = counter.wrapping_add(1);
                if counter % 10_000_000 == 0 {
                    println!("  CHILD: Still alive...");
                }
                // Yield to let parent run
                if counter % 100_000 == 0 {
                    let _ = yield_now();
                }
            }
        }
        Ok(ForkResult::Parent(child_pid)) => {
            // Parent process
            let child_pid_i32 = child_pid.raw() as i32;
            println!("  PARENT: Forked child with PID {}", child_pid_i32);

            // Small delay to let child start
            println!("  PARENT: Waiting for child to start...");
            for i in 0..5 {
                println!("  PARENT: yield {}", i);
                let _ = yield_now();
            }
            println!("  PARENT: Done waiting, about to send signal");

            // Send SIGTERM to child
            println!("  PARENT: Sending SIGTERM to child");
            let ret = kill(child_pid_i32, SIGTERM);
            if ret.is_ok() {
                println!("  PARENT: kill() syscall succeeded");
            } else {
                println!("  PARENT: kill() failed");
                std::process::exit(1);
            }

            // Wait for child to actually terminate using waitpid
            println!("  PARENT: Waiting for child to terminate...");
            let mut status: i32 = 0;
            let result = waitpid(child_pid_i32, &mut status, 0).unwrap();

            if result.raw() as i32 == child_pid_i32 {
                // Check if child was terminated by signal
                // WTERMSIG: status & 0x7f
                let termsig = wtermsig(status);
                if termsig == SIGTERM {
                    println!("  PARENT: Child terminated by SIGTERM!");
                    println!("SIGNAL_KILL_TEST_PASSED");
                } else if termsig != 0 {
                    println!("  PARENT: Child terminated by wrong signal: {}", termsig);
                    std::process::exit(2);
                } else {
                    // Child exited normally (WIFEXITED)
                    let exit_code = (status >> 8) & 0xff;
                    println!(
                        "  PARENT: Child exited normally (not by signal), exit code: {}",
                        exit_code
                    );
                    std::process::exit(3);
                }
            } else {
                println!(
                    "  PARENT: waitpid returned unexpected value: {}",
                    result.raw() as i32
                );
                std::process::exit(4);
            }

            println!("  PARENT: Test complete, exiting");
            std::process::exit(0);
        }
        Err(_) => {
            println!("fork failed");
            std::process::exit(1);
        }
    }
}
