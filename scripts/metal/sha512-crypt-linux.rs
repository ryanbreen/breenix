//! Linux staging helper cross-compiled without a host C library. Same hash code
//! and stdin protocol as sha512-crypt.rs; raw Linux x86-64 read/write/exit ABI.
#![no_std]
#![no_main]
#[path = "@SHA512@"]
pub mod sha512;
#[path = "@CRYPT@"]
pub mod sha512_crypt;
use core::arch::{asm, global_asm};

global_asm!(".global _start", "_start:", "and rsp, -16", "call run");

unsafe fn syscall(number: usize, fd: usize, buffer: *const u8, size: usize) -> isize {
    let result: isize;
    asm!("syscall", inlateout("rax") number => result, in("rdi") fd,
         in("rsi") buffer, in("rdx") size, out("rcx") _, out("r11") _, options(nostack));
    result
}

fn exit(status: usize) -> ! {
    unsafe { asm!("syscall", in("rax") 60usize, in("rdi") status, options(noreturn)); }
}

#[no_mangle]
pub extern "C" fn run() -> ! {
    let mut input = [0u8; 1024];
    let mut length = 0;
    while length < input.len() {
        let count = unsafe { syscall(0, 0, input[length..].as_ptr(), input.len() - length) };
        if count < 0 { exit(1); }
        if count == 0 { break; }
        length += count as usize;
    }
    let result = (|| {
        let split = input[..length].iter().position(|b| *b == 0)?;
        if split == 0 || split > 256 { return None; }
        let setting = core::str::from_utf8(&input[split + 1..length]).ok()?;
        let mut output = [0; sha512_crypt::MAX_OUTPUT];
        let size = sha512_crypt::sha512_crypt(&input[..split], setting, &mut output).ok()?;
        let mut sent = 0;
        while sent < size {
            let count = unsafe { syscall(1, 1, output[sent..].as_ptr(), size - sent) };
            if count <= 0 { return None; }
            sent += count as usize;
        }
        Some(())
    })();
    for byte in &mut input { unsafe { core::ptr::write_volatile(byte, 0); } }
    if result.is_none() {
        let message = b"console hash generation failed\n";
        unsafe { syscall(1, 2, message.as_ptr(), message.len()); }
        exit(1);
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { exit(1) }
