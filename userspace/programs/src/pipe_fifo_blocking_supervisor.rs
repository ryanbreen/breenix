//! Private-image init for the #813 boot-only gate. Score the actual reaped worker.
use libbreenix::process::{self, ForkResult};
fn main() {
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let status = match process::fork() {
        Ok(ForkResult::Child) => {
            let _ = process::exec(b"/bin/pipe_fifo_blocking_oracle\0");
            process::exit(127);
        }
        Ok(ForkResult::Parent(pid)) => {
            let mut status = -1;
            match process::waitpid(pid.raw() as i32, &mut status, 0) {
                Ok(reaped) if reaped == pid && process::wifexited(status) => {
                    process::wexitstatus(status)
                }
                _ => 126,
            }
        }
        Err(_) => 125,
    };
    println!("[PIPE_WRITE_RESULT:{}:status={}]", arch, status);
    loop {
        let _ = libbreenix::time::sleep_ms(1000);
    }
}
