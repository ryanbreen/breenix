//! Executable entry for the hosted dynamic link. Build as a standalone object.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

#[unsafe(naked)]
#[no_mangle]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mov rdi, rsp",
        "and rsp, -16",
        "call {start}",
        start = sym start,
    );
}

extern "C" fn start(stack: *const usize) -> ! {
    extern "C" {
        fn __breenix_libc_init(stack: *const usize);
        fn main(argc: i32, argv: *const *const u8, envp: *const *const u8) -> i32;
        fn exit(status: i32) -> !;
    }
    // SAFETY: _start forwards the initial kernel stack.
    unsafe { __breenix_libc_init(stack) };
    // SAFETY: argc occupies the first word of the initial stack.
    let argc = unsafe { stack.read() };
    // SAFETY: argv occupies the next argc words, followed by its NULL and envp.
    let argv = unsafe { stack.add(1) }.cast::<*const u8>();
    // SAFETY: the stack contains argc + 1 argv entries including the NULL.
    let envp = unsafe { argv.add(argc + 1) };
    // SAFETY: main is linked into the executable and the vectors are terminated.
    let status = unsafe { main(argc as i32, argv, envp) };
    // SAFETY: libc was initialized before entering main.
    unsafe { exit(status) };
}
