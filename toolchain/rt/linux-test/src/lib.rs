//! Runtime for evaluating bcc programs in the Linux build container.
//! Its results are development checks and carry no Breenix suite evidence.

#![no_std]
// The executable check links the archive. Cargo's std test harness owns its
// own process entry and panic handler.
#![cfg(not(test))]
#![feature(c_variadic)]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    target_pointer_width = "64"
)))]
compile_error!("linux-test requires the x86-64 or aarch64 Linux LP64 ABI");

use core::arch::{asm, global_asm};
use core::ffi::{c_char, c_int, c_long, c_uint, c_void};
use core::{ptr, slice};

#[cfg(target_arch = "x86_64")]
// AMD64 psABI §3.4.1 and §3.2.3 describe the initial stack and the C call.
// SAFETY: The entry point reads argc/argv/envp from that stack, aligns RSP
// for both calls, and hands main's integer result to the non-returning exit.
global_asm!(
    ".section .text._start,\"ax\",@progbits",
    ".globl _start",
    ".type _start,@function",
    "_start:",
    "xor ebp, ebp",
    "mov rdi, [rsp]",
    "lea rsi, [rsp + 8]",
    "lea rdx, [rsi + rdi * 8 + 8]",
    "and rsp, -16",
    "call main",
    "mov edi, eax",
    "call {exit}",
    ".size _start, . - _start",
    exit = sym exit,
);

// AAPCS64 §6.4.5 requires a 16-byte aligned stack at public interfaces.
// The Linux initial stack starts with argc followed by argv and envp.
// SAFETY: The entry point derives argv and envp from the initial stack and
// passes main's result to the non-returning exit function.
#[cfg(target_arch = "aarch64")]
global_asm!(
    ".section .text._start,\"ax\",@progbits",
    ".globl _start",
    ".type _start,@function",
    "_start:",
    "ldr x0, [sp]",
    "add x1, sp, #8",
    "add x2, x1, x0, lsl #3",
    "add x2, x2, #8",
    "bl main",
    "bl {exit}",
    ".size _start, . - _start",
    exit = sym exit,
);

#[repr(transparent)]
struct Stream(c_int);

static STDOUT: Stream = Stream(1);

// The C declaration is an exported FILE * object. fwrite recognizes this stream.
#[no_mangle]
static mut stdout: *const Stream = &STDOUT;

#[cfg(target_arch = "x86_64")]
const SYS_WRITE: isize = 1;
#[cfg(target_arch = "x86_64")]
const SYS_EXIT: isize = 60;
// libs/libbreenix/src/syscall.rs:281,301 (E25 ruling 40).
#[cfg(target_arch = "aarch64")]
const SYS_WRITE: isize = 64;
#[cfg(target_arch = "aarch64")]
const SYS_EXIT: isize = 93;
const EINTR: isize = 4;

#[cfg(target_arch = "x86_64")]
// The AMD64 psABI appendix A.2.1 describes Linux's syscall ABI.
unsafe fn sys_write(fd: c_int, data: *const u8, len: usize) -> isize {
    let result;
    // SAFETY: The caller supplies a buffer readable for len bytes. The syscall
    // reads it synchronously and preserves registers other than RAX, RCX and R11.
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") SYS_WRITE => result,
            in("rdi") fd,
            in("rsi") data,
            in("rdx") len,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    result
}

#[cfg(target_arch = "aarch64")]
unsafe fn sys_write(fd: c_int, data: *const u8, len: usize) -> isize {
    let result;
    // SAFETY: The caller supplies a readable buffer, consumed synchronously.
    unsafe {
        asm!(
            "svc #0",
            in("x8") SYS_WRITE,
            inlateout("x0") fd as isize => result,
            in("x1") data,
            in("x2") len,
            options(nostack),
        );
    }
    result
}

#[no_mangle]
unsafe extern "C" fn write(fd: c_int, data: *const c_void, len: usize) -> isize {
    // SAFETY: The C caller supplies a buffer readable for len bytes.
    let result = unsafe { sys_write(fd, data.cast(), len) };
    result.max(-1)
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
extern "C" fn exit(status: c_int) -> ! {
    // SAFETY: The Linux exit syscall takes only the status. If the syscall
    // returns unexpectedly, ud2 terminates execution before it can fall through.
    unsafe {
        asm!(
            "syscall",
            "ud2",
            in("rax") SYS_EXIT,
            in("rdi") status,
            options(nostack, noreturn),
        );
    }
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
extern "C" fn exit(status: c_int) -> ! {
    // SAFETY: The exit syscall consumes the status and does not return;
    // brk traps if it unexpectedly does.
    unsafe {
        asm!("svc #0", "brk #0", in("x8") SYS_EXIT, in("x0") status,
             options(nostack, noreturn));
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    exit(127)
}

fn write_stdout(bytes: &[u8]) -> usize {
    let mut remaining = bytes;
    while !remaining.is_empty() {
        // SAFETY: The slice stays readable for the duration of the syscall.
        let written = unsafe { sys_write(STDOUT.0, remaining.as_ptr(), remaining.len()) };
        match written {
            n if n == -EINTR => continue,
            n if n <= 0 => break,
            n => remaining = &remaining[n as usize..],
        }
    }
    bytes.len() - remaining.len()
}

unsafe fn c_bytes<'a>(string: *const c_char) -> &'a [u8] {
    let start = string.cast::<u8>();
    let mut len = 0;
    loop {
        // SAFETY: The caller supplies a readable C string through its terminator.
        let byte = unsafe { start.wrapping_add(len).read() };
        if byte == 0 {
            break;
        }
        len += 1;
    }
    // SAFETY: The caller keeps the string readable and unchanged for 'a.
    unsafe { slice::from_raw_parts(start, len) }
}

#[no_mangle]
unsafe extern "C" fn puts(string: *const c_char) -> c_int {
    // SAFETY: The caller supplies a readable NUL-terminated string for this call.
    let bytes = unsafe { c_bytes(string) };
    if write_stdout(bytes) == bytes.len() && write_stdout(b"\n") == 1 {
        0
    } else {
        -1
    }
}

#[no_mangle]
unsafe extern "C" fn fwrite(
    data: *const c_void,
    size: usize,
    count: usize,
    stream: *const Stream,
) -> usize {
    let Some(len) = size.checked_mul(count) else {
        return 0;
    };
    if len == 0 || len > isize::MAX as usize || !ptr::eq(stream, &STDOUT) {
        return 0;
    }
    // SAFETY: A valid fwrite caller supplies a readable buffer of size * count
    // bytes for this call. The length is nonzero and representable by isize.
    let bytes = unsafe { slice::from_raw_parts(data.cast(), len) };
    write_stdout(bytes) / size
}

fn decimal(mut value: u64, negative: bool, buffer: &mut [u8; 21]) -> &[u8] {
    let mut start = buffer.len();
    loop {
        start -= 1;
        buffer[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    if negative {
        start -= 1;
        buffer[start] = b'-';
    }
    &buffer[start..]
}

// N1570 §7.21.6.1: decimal signed/unsigned, ptrdiff_t, C strings and literal %.
#[no_mangle]
unsafe extern "C" fn printf(format: *const c_char, mut args: ...) -> c_int {
    // SAFETY: The caller supplies a readable NUL-terminated format for this call.
    let mut format = unsafe { c_bytes(format) };
    let mut written = 0usize;
    while let Some(index) = format.iter().position(|&byte| byte == b'%') {
        let (literal, conversion) = format.split_at(index);
        if write_stdout(literal) != literal.len() {
            return -1;
        }
        written = written.saturating_add(literal.len());

        let mut digits = [0; 21];
        let (bytes, rest): (&[u8], _) = match conversion {
            [b'%', b'd', rest @ ..] => {
                // SAFETY: %d requires an int argument under the C variadic ABI.
                let value = unsafe { args.arg::<c_int>() };
                (
                    decimal(u64::from(value.unsigned_abs()), value < 0, &mut digits),
                    rest,
                )
            }
            [b'%', b'u', rest @ ..] => {
                // SAFETY: %u requires an unsigned int argument.
                let value = unsafe { args.arg::<c_uint>() };
                (decimal(u64::from(value), false, &mut digits), rest)
            }
            [b'%', b'l' | b't', b'd', rest @ ..] => {
                // SAFETY: %ld takes long; Breenix's LP64 ptrdiff_t is also long.
                let value = unsafe { args.arg::<c_long>() };
                (decimal(value.unsigned_abs(), value < 0, &mut digits), rest)
            }
            [b'%', b's', rest @ ..] => {
                // SAFETY: %s requires a pointer to a C string.
                let string = unsafe { args.arg::<*const c_char>() };
                // SAFETY: The caller keeps the %s string readable for this call.
                (unsafe { c_bytes(string) }, rest)
            }
            [b'%', b'%', rest @ ..] => (b"%", rest),
            _ => return -1,
        };
        if write_stdout(bytes) != bytes.len() {
            return -1;
        }
        written = written.saturating_add(bytes.len());
        if written > c_int::MAX as usize {
            return -1;
        }
        format = rest;
    }
    if write_stdout(format) != format.len() {
        return -1;
    }
    c_int::try_from(written + format.len()).unwrap_or(-1)
}
