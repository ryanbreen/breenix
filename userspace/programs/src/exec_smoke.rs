//! Exec smoke launcher — the boot-path execve caller.
//!
//! aarch64 userland otherwise creates every process with sys_spawn, so without this program the
//! aarch64 exec path (sys_exec_aarch64 -> ProcessManager::exec_process_with_argv -> ExecSchedCommit)
//! has no runtime coverage on any gate. This process replaces itself with /bin/exec_smoke_target;
//! the marker the gate asserts on is printed by the target, i.e. only a completed exec can produce it.

use libbreenix::process::execv;

fn main() {
    println!("[EXEC_SMOKE:LAUNCH]");
    let path = b"/bin/exec_smoke_target\0";
    let arg0 = b"exec_smoke_target\0".as_ptr();
    let arg1 = b"smoke\0".as_ptr();
    let argv: [*const u8; 3] = [arg0, arg1, std::ptr::null()];

    let _ = execv(path, argv.as_ptr());

    // execv only returns on failure.
    println!("[EXEC_SMOKE:EXEC_FAILED]");
    std::process::exit(1);
}
