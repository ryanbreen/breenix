//! The public shutdown commands ask PID 1; sudo supplies root credentials.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub fn run(action: &str) -> ! {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result: Result<(), String> = (|| {
        if args.as_slice() == ["--kernel-check"] {
            // Invalid magic makes this a non-destructive privilege probe even
            // when run as root. A user must get EPERM before argument validation.
            #[cfg(target_arch = "x86_64")]
            let number = 169;
            #[cfg(target_arch = "aarch64")]
            let number = 142;
            let ret = unsafe { libbreenix::syscall::raw::syscall3(number, 0, 0, 0) } as i64;
            return Err(if ret == -1 { "permission denied".into() } else { format!("kernel check returned {}", ret) });
        }
        if !args.is_empty() { return Err(format!("usage: {}", action)); }
        let mut socket = UnixStream::connect(libbreenix::services::SOCKET).map_err(|e| e.to_string())?;
        socket.write_all(format!("shutdown {}\n", action).as_bytes()).map_err(|e| e.to_string())?;
        let mut reply = String::new();
        BufReader::new(socket).read_line(&mut reply).map_err(|e| e.to_string())?;
        if reply.trim() != format!("ok shutdown {} accepted", action) {
            return Err(reply.trim().strip_prefix("error ").unwrap_or("init refused shutdown").into());
        }
        Ok(())
    })();
    match result {
        Ok(()) => { println!("{}: shutdown accepted", action); std::process::exit(0); }
        Err(why) => { eprintln!("{}: {}", action, why); std::process::exit(1); }
    }
}
