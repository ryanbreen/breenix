//! Basic exec test - calls execve to replace current image with exec_target
//! Expected behavior: prints EXEC_OK (from exec_target) and exits with 0

#![no_std]
#![no_main]

include!("libbreenix.rs");

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Try to exec into exec_target
    let path = b"exec_target\0";
    let argv = [core::ptr::null::<u8>()];   // argv[0] = NULL for now
    
    let ret = unsafe { sys_execve(path.as_ptr(), argv.as_ptr()) };
    
    // We reach here only on failure
    unsafe { 
        sys_write(1, b"EXEC_FAIL\n");
        sys_exit(ret as i32);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        sys_write(1, b"exec_basic: Panic!\n");
        sys_exit(1);
    }
}