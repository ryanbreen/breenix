//! Target program for exec testing - prints EXEC_OK and exits
//! This program is loaded by execve() tests to prove the exec syscall works

#![no_std]
#![no_main]

include!("libbreenix.rs");

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Print EXEC_OK marker to prove exec succeeded
    unsafe { 
        sys_write(1, b"EXEC_OK\n");
    }
    
    // Exit successfully
    unsafe { 
        sys_exit(0); 
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        sys_write(1, b"exec_target: Panic!\n");
        sys_exit(1);
    }
}