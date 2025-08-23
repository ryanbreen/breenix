use std::{
    env,
    fs,
    path::PathBuf,
    process::{self, Command},
};

use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};

fn main() {
    // Allow overriding OVMF firmware paths via environment for CI/DEBUG builds
    let ovmf_code = if let Ok(path) = env::var("BREENIX_OVMF_CODE_PATH") {
        PathBuf::from(path)
    } else {
        let prebuilt = Prebuilt::fetch(Source::LATEST, "target/ovmf").unwrap();
        prebuilt.get_file(Arch::X64, FileType::Code)
    };
    let ovmf_vars_src = if let Ok(path) = env::var("BREENIX_OVMF_VARS_PATH") {
        PathBuf::from(path)
    } else {
        let prebuilt = Prebuilt::fetch(Source::LATEST, "target/ovmf").unwrap();
        prebuilt.get_file(Arch::X64, FileType::Vars)
    };
    // QEMU requires VARS to be writable. Copy to a temp file to ensure write access on CI.
    let vars_dst: PathBuf = {
        let mut p = env::temp_dir();
        p.push("OVMF_VARS.fd");
        // Best effort copy; if it fails we'll still try original path
        let _ = fs::copy(&ovmf_vars_src, &p);
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
    // Canonical pflash wiring for OVMF: CODE and writable VARS copy
    qemu.args(["-pflash", &ovmf_code.display().to_string()]);
    qemu.args(["-pflash", &vars_dst.display().to_string()]);
    // Attach kernel disk image. Default to virtio; allow override via env.
    // On CI we export BREENIX_QEMU_STORAGE=ide to favor OVMF boot discovery.
    let storage_mode = env::var("BREENIX_QEMU_STORAGE").unwrap_or_else(|_| "virtio".to_string());
    match storage_mode.as_str() {
        "ide" => {
            // Minimal, known-good IDE attach; no explicit AHCI controller
            qemu.args([
                "-drive",
                &format!("if=ide,format=raw,media=disk,file={},index=0", uefi_img.display()),
            ]);
            eprintln!("[qemu-uefi] Storage: IDE (index=0)");
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
        "-boot", "strict=on",
        "-no-reboot",
        "-no-shutdown",
    ]);
    // Deterministic guest-driven exit for CI via isa-debug-exit on port 0xF4
    qemu.args([
        "-device",
        "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ]);
    // Optional debug log and firmware debug console capture
    if let Ok(log_path) = env::var("BREENIX_QEMU_LOG_PATH") {
        qemu.args(["-d", "guest_errors", "-D", &log_path]);
        eprintln!("[qemu-uefi] QEMU debug log at {}", log_path);
    }
    // If a file path is provided, route firmware debug console (0x402) to file.
    if let Ok(path) = env::var("BREENIX_QEMU_DEBUGCON_FILE") {
        qemu.args(["-debugcon", &format!("file:{}", path)]);
        qemu.args(["-global", "isa-debugcon.iobase=0x402"]);
        eprintln!("[qemu-uefi] Debug console (0x402) -> file: {}", path);
    } else if env::var("BREENIX_QEMU_DEBUGCON").ok().as_deref() == Some("1") {
        // Fallback: map debug console to stdio if explicitly requested
        qemu.args(["-chardev", "stdio,id=ovmf", "-device", "isa-debugcon,iobase=0x402,chardev=ovmf"]);
        eprintln!("[qemu-uefi] Debug console (0x402) -> stdio enabled");
    }
    // Hint firmware to route stdout to serial (fw_cfg toggle; ignored if unsupported)
    qemu.args(["-fw_cfg", "name=opt/org.tianocore/StdoutToSerial,string=1"]);
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