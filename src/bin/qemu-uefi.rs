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
    // Verify UEFI image exists
    let uefi_img = PathBuf::from(env!("UEFI_IMAGE"));
    if !uefi_img.exists() {
        eprintln!("[qemu-uefi] UEFI image missing: {}", uefi_img.display());
    } else {
        eprintln!("[qemu-uefi] Using UEFI image: {} ({} bytes)", uefi_img.display(), fs::metadata(&uefi_img).map(|m| m.len()).unwrap_or(0));
    }
    qemu.args([
        "-drive",
        &format!("format=raw,if=pflash,unit=0,readonly=on,file={}", ovmf_code.display()),
        "-drive",
        &format!("format=raw,if=pflash,unit=1,file={}", vars_dst.display()),
    ]);
    // Attach kernel disk image. Default to virtio; allow override via env.
    // On CI we export BREENIX_QEMU_STORAGE=ide to favor OVMF boot discovery.
    let storage_mode = env::var("BREENIX_QEMU_STORAGE").unwrap_or_else(|_| "virtio".to_string());
    match storage_mode.as_str() {
        "ide" => {
            qemu.args([
                "-drive",
                &format!("if=none,id=hd,format=raw,media=disk,file={}", uefi_img.display()),
                "-device", "ich9-ahci,id=sata",
                "-device", "ide-hd,drive=hd,bus=sata.0,bootindex=0",
            ]);
            eprintln!("[qemu-uefi] Storage: IDE (AHCI)");
        }
        _ => {
            qemu.args([
                "-drive",
                &format!("if=none,id=hd,format=raw,media=disk,file={}", uefi_img.display()),
                "-device", "virtio-blk-pci,drive=hd,bootindex=0",
            ]);
            eprintln!("[qemu-uefi] Storage: virtio-blk");
        }
    }
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
    // Deterministic guest-driven exit for CI via isa-debug-exit on port 0xF4
    qemu.args([
        "-device",
        "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ]);
    // Optional debug log and early-debug console to stdout
    if let Ok(log_path) = env::var("BREENIX_QEMU_LOG_PATH") {
        qemu.args(["-d", "guest_errors", "-D", &log_path]);
        eprintln!("[qemu-uefi] QEMU debug log at {}", log_path);
    }
    if env::var("BREENIX_QEMU_DEBUGCON").ok().as_deref() == Some("1") {
        // Map debug console (I/O port 0x402) to stdout for very-early bytes
        qemu.args(["-chardev", "stdio,id=dbg", "-device", "isa-debugcon,iobase=0x402,chardev=dbg"]);
        eprintln!("[qemu-uefi] Debug console (0x402) -> stdio enabled");
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