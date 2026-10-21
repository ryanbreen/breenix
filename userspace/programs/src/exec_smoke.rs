//! Exec smoke launcher — the boot-path execve caller.
//!
//! This launcher exercises execve by replacing itself with /usr/bin/exec_smoke_target.
//! The boot gate's marker is printed by the target after exec.


use libbreenix::process::execv;

fn main() {
    println!("[EXEC_SMOKE:LAUNCH]");
    let path = b"/usr/bin/exec_smoke_target\0";
    let arg0 = b"exec_smoke_target\0".as_ptr();
    let arg1 = b"smoke\0".as_ptr();
    let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

    let _ = execv(path, argv.as_ptr());

    // execv only returns on failure.
    println!("[EXEC_SMOKE:EXEC_FAILED]");
    std::process::exit(1);
}
