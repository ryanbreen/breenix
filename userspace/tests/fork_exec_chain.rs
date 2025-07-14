//! Fork+exec chain test - parent forks, child execs into exec_target
//! Expected behavior: 
//! - Parent prints PARENT_OK
//! - Child prints EXEC_OK (from exec_target) exactly once

#![no_std]
#![no_main]

include!("libbreenix.rs");

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let pid = unsafe { sys_fork() };
    
    if pid == 0 {
        // Child process: replace image with exec_target
        let path = b"exec_target\0";
        let argv = [core::ptr::null::<u8>()];
        
        unsafe { sys_execve(path.as_ptr(), argv.as_ptr()); }
        
        // Should not reach here on successful exec
        unsafe { sys_exit(127); }   
    } else {
        // Parent process: simple confirmation
        unsafe { 
            sys_write(1, b"PARENT_OK\n");
            sys_exit(0);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        sys_write(1, b"fork_exec_chain: Panic!\n");
        sys_exit(1);
    }
}