//! Runtime proof for CLONE_VM sibling lifetime across parent exec.
//!
//! The child remains alive in the parent's old address space while the parent
//! attempts exec. Fixed ARM64 kernels must reject that exec with EAGAIN.

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::memory;
use libbreenix::process;
use std::ptr;

const SHARED_ALIVE_OFFSET: usize = 0;
const SHARED_COMMAND_OFFSET: usize = 8;
const SHARED_TID_OFFSET: usize = 16;
const CHILD_STACK_SIZE: usize = 64 * 1024;
const CHILD_SPIN_LIMIT: u64 = 250_000_000;

extern "C" {
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
}

unsafe fn raw_msg(msg: &[u8]) {
    write(2, msg.as_ptr(), msg.len());
}

unsafe fn sys_yield() {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!("int 0x80", in("rax") 24u64, options(nostack));
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!("svc #0", in("x8") 124u64, in("x0") 0u64, options(nostack));
}

unsafe fn thread_exit(code: u64) -> ! {
    #[cfg(target_arch = "x86_64")]
    core::arch::asm!(
        "int 0x80",
        "2:",
        "pause",
        "jmp 2b",
        in("rax") 60u64,
        in("rdi") code,
        options(noreturn),
    );
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        "2:",
        "yield",
        "b 2b",
        in("x8") 93u64,
        in("x0") code,
        options(noreturn),
    );
}

extern "C" fn child_fn(arg: *mut u8) -> *mut u8 {
    unsafe {
        raw_msg(b"CLONEVM_EXEC_TEST: child spinning (alive)\n");

        let alive = arg.add(SHARED_ALIVE_OFFSET) as *mut u64;
        let command = arg.add(SHARED_COMMAND_OFFSET) as *mut u64;
        core::ptr::write_volatile(alive, 1);

        for i in 0..CHILD_SPIN_LIMIT {
            if core::ptr::read_volatile(command) == 2 {
                raw_msg(b"CLONEVM_EXEC_TEST: child release observed\n");
                thread_exit(0);
            }

            core::ptr::write_volatile(alive, i | 1);
            if i % 1024 == 0 {
                sys_yield();
            }
        }

        raw_msg(b"CLONEVM_EXEC_TEST: child spin limit reached\n");
        thread_exit(0);
    }
}

fn errno_number(errno: Errno) -> i64 {
    errno as i64
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--second-stage") {
        unsafe {
            raw_msg(b"CLONEVM_EXEC_TEST: second stage calling exec\n");
        }
        match process::exec(b"/bin/simple_exit\0") {
            Ok(_) => unreachable!(),
            Err(Error::Os(errno)) => {
                eprintln!(
                    "CLONEVM_EXEC_TEST: second stage exec returned errno={}",
                    errno_number(errno)
                );
                std::process::exit(1);
            }
        }
    }

    unsafe {
        raw_msg(b"CLONEVM_EXEC_TEST: start\n");

        let stack = match memory::mmap(
            core::ptr::null_mut(),
            CHILD_STACK_SIZE,
            3,
            0x22,
            -1,
            0,
        ) {
            Ok(ptr) => ptr,
            Err(_) => {
                raw_msg(b"CLONEVM_EXEC_TEST: ERROR stack mmap failed\n");
                std::process::exit(1);
            }
        };

        let shared = match memory::mmap(core::ptr::null_mut(), 4096, 3, 0x22, -1, 0) {
            Ok(ptr) => ptr,
            Err(_) => {
                raw_msg(b"CLONEVM_EXEC_TEST: ERROR shared mmap failed\n");
                std::process::exit(1);
            }
        };

        let alive = shared.add(SHARED_ALIVE_OFFSET) as *mut u64;
        let command = shared.add(SHARED_COMMAND_OFFSET) as *mut u64;
        let tid_addr = shared.add(SHARED_TID_OFFSET) as *mut u32;
        core::ptr::write_volatile(alive, 0);
        core::ptr::write_volatile(command, 0);
        core::ptr::write_volatile(tid_addr, 0xFFFF);

        let stack_top = (stack as usize + CHILD_STACK_SIZE) & !0xF;
        let flags: u64 = 0x00000100 | 0x00000400 | 0x00200000 | 0x01000000;

        raw_msg(b"CLONEVM_EXEC_TEST: parent calling clone\n");
        let ret: i64;
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!(
            "int 0x80",
            in("rax") 56u64,
            in("rdi") flags,
            in("rsi") stack_top as u64,
            in("rdx") child_fn as u64,
            in("r10") shared as u64,
            in("r8") tid_addr as u64,
            lateout("rax") ret,
            options(nostack),
        );
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!(
            "svc #0",
            in("x8") 220u64,
            inlateout("x0") flags as u64 => ret,
            in("x1") stack_top as u64,
            in("x2") child_fn as u64,
            in("x3") shared as u64,
            in("x4") tid_addr as u64,
            options(nostack),
        );

        if ret < 0 {
            raw_msg(b"CLONEVM_EXEC_TEST: ERROR clone failed\n");
            std::process::exit(1);
        }

        raw_msg(b"CLONEVM_EXEC_TEST: parent waiting for live child\n");
        for _ in 0..2_000_000u64 {
            if core::ptr::read_volatile(alive) != 0 {
                break;
            }
            sys_yield();
        }

        if core::ptr::read_volatile(alive) == 0 {
            raw_msg(b"CLONEVM_EXEC_TEST: ERROR child did not report alive\n");
            std::process::exit(1);
        }

        let program = b"/usr/local/test/bin/clonevm_exec_test\0";
        let arg0 = b"clonevm_exec_test\0";
        let arg1 = b"--second-stage\0";
        let argv: [*const u8; 3] = [arg0.as_ptr(), arg1.as_ptr(), ptr::null()];

        raw_msg(b"CLONEVM_EXEC_TEST: parent calling exec\n");
        match process::execv(program, argv.as_ptr()) {
            Ok(_) => unreachable!(),
            Err(Error::Os(errno)) => {
                let errno_value = errno_number(errno);
                eprintln!("CLONEVM_EXEC_TEST: exec returned errno={}", errno_value);
                if errno == Errno::EAGAIN {
                    raw_msg(b"CLONEVM_EXEC_TEST: fixed path observed EAGAIN\n");
                    core::ptr::write_volatile(command, 2);

                    for _ in 0..2_000_000u64 {
                        if core::ptr::read_volatile(tid_addr) == 0 {
                            break;
                        }
                        sys_yield();
                    }

                    if core::ptr::read_volatile(tid_addr) == 0 {
                        raw_msg(b"CLONEVM_EXEC_TEST: child exited after release\n");
                        raw_msg(b"CLONEVM_EXEC_TEST: PASS\n");
                        std::process::exit(0);
                    }

                    raw_msg(b"CLONEVM_EXEC_TEST: ERROR child did not exit after release\n");
                    std::process::exit(1);
                }

                raw_msg(b"CLONEVM_EXEC_TEST: ERROR unexpected exec errno\n");
                std::process::exit(1);
            }
        }
    }
}
