use std::{
    env,
    process::{self, Command},
};

fn main() {
    let mut qemu = Command::new("qemu-system-x86_64");
    qemu.arg("-drive");
    qemu.arg(format!("format=raw,file={}", env!("BIOS_IMAGE")));

    // Optional debug log support
    if let Ok(log_path) = env::var("BREENIX_QEMU_LOG_PATH") {
        let debug_flags = env::var("BREENIX_QEMU_DEBUG_FLAGS")
            .unwrap_or_else(|_| "guest_errors".to_string());
        qemu.args(["-d", &debug_flags, "-D", &log_path]);
        eprintln!("[qemu-bios] Debug log: {} (flags: {})", log_path, debug_flags);
    }

    // Forward any additional command-line arguments to QEMU
    let extra_args: Vec<String> = env::args().skip(1).collect();
    if !extra_args.is_empty() {
        eprintln!("[qemu-bios] Extra args: {:?}", extra_args);
        qemu.args(&extra_args);
    }

    let exit_status = qemu.status().unwrap();
    process::exit(exit_status.code().unwrap_or(-1));
}