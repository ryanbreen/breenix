use std::{
    env,
    fs,
    path::PathBuf,
    process::{self, Command},
};

use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};

fn main() {
    let prebuilt =
        Prebuilt::fetch(Source::LATEST, "target/ovmf").unwrap();
    let ovmf_code = prebuilt.get_file(Arch::X64, FileType::Code);
    let ovmf_vars = prebuilt.get_file(Arch::X64, FileType::Vars);
    // QEMU requires VARS to be writable. Copy to a temp file to ensure write access on CI.
    let vars_dst: PathBuf = {
        let mut p = env::temp_dir();
        p.push("OVMF_VARS.fd");
        // Best effort copy; if it fails we'll still try original path
        let _ = fs::copy(&ovmf_vars, &p);
        p
    };
    let mut qemu = Command::new("qemu-system-x86_64");
    qemu.args([
        "-drive",
        &format!("format=raw,if=pflash,unit=0,readonly=on,file={}", ovmf_code.display()),
        "-drive",
        &format!("format=raw,if=pflash,unit=1,file={}", vars_dst.display()),
        // Attach kernel disk image as virtio-blk device for OVMF to discover
        "-drive",
        &format!("if=none,id=hd,format=raw,file={}", env!("UEFI_IMAGE")),
        "-device", "virtio-blk-pci,drive=hd",
    ]);
    // Improve CI capture and stability
    qemu.args([
        "-machine", "accel=tcg",
        "-cpu", "qemu64",
        "-smp", "1",
        "-m", "512",
        "-nographic",
        "-monitor", "none",
        "-no-reboot",
        "-no-shutdown",
    ]);
    // Optional debug log
    if let Ok(log_path) = env::var("BREENIX_QEMU_LOG_PATH") {
        qemu.args(["-d", "guest_errors", "-D", &log_path]);
        eprintln!("[qemu-uefi] QEMU debug log at {}", log_path);
    }
    // Forward any additional command-line arguments to QEMU (runner may supply -serial ...)
    let extra_args: Vec<String> = env::args().skip(1).collect();
    if !extra_args.is_empty() {
        eprintln!("[qemu-uefi] Extra args: {:?}", extra_args);
        qemu.args(&extra_args);
    }
    eprintln!("[qemu-uefi] Launching QEMU...");
    let exit_status = qemu.status().unwrap();
    process::exit(exit_status.code().unwrap_or(-1));
}