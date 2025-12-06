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
        let p = PathBuf::from(path);
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.contains("secboot") || name.contains(".ms") {
            eprintln!("[qemu-uefi] Refusing Secure Boot firmware in CI: {}", p.display());
            process::exit(2);
        }
        p
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
    // Sanity log OVMF selection and sizes when env overrides are used
    if env::var("BREENIX_OVMF_CODE_PATH").is_ok() || env::var("BREENIX_OVMF_VARS_PATH").is_ok() {
        let code_path = ovmf_code.canonicalize().unwrap_or(ovmf_code.clone());
        let vars_path = ovmf_vars_src.canonicalize().unwrap_or(ovmf_vars_src.clone());
        let (mut csize, mut vsize) = (0u64, 0u64);
        if let Ok(m) = fs::metadata(&code_path) { csize = m.len(); }
        if let Ok(m) = fs::metadata(&vars_path) { vsize = m.len(); }
        let family = if csize >= 4_000_000 && vsize >= 4_000_000 { "4M" } else { "2M-ish" };
        eprintln!(
            "[qemu-uefi] OVMF selected: CODE={} ({} bytes), VARS={} ({} bytes) [{} family]",
            code_path.display(), csize, vars_path.display(), vsize, family
        );
    }
    let mut qemu = Command::new("qemu-system-x86_64");
    // Verify UEFI image exists
    let uefi_img = PathBuf::from(env!("UEFI_IMAGE"));
    if !uefi_img.exists() {
        eprintln!("[qemu-uefi] UEFI image missing: {}", uefi_img.display());
    } else {
        eprintln!("[qemu-uefi] Using UEFI image: {} ({} bytes)", uefi_img.display(), fs::metadata(&uefi_img).map(|m| m.len()).unwrap_or(0));
    }
    // Optional: print UEFI image path and exit (for CI ESP precheck)
    if env::var("BREENIX_PRINT_UEFI_IMAGE").ok().as_deref() == Some("1") {
        let canon = uefi_img.canonicalize().unwrap_or(uefi_img.clone());
        println!("UEFI_IMAGE={}", canon.display());
        process::exit(0);
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
            // Use disable-modern=on to force legacy (virtio 0.9) interface
            // Our VirtIO driver only supports the legacy I/O port interface
            qemu.args([
                "-drive",
                &format!("if=none,id=hd,format=raw,media=disk,file={}", uefi_img.display()),
                "-device", "virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off",
            ]);
            eprintln!("[qemu-uefi] Storage: virtio-blk (legacy mode)");
        }
    }

    // Attach test binaries disk (second VirtIO device, index 1)
    // MANDATORY - disk loading is always required
    if storage_mode == "virtio" {
        // Determine project root by walking up from executable directory
        let exe_path = env::current_exe().expect("Failed to get executable path");
        let mut project_root = exe_path.parent().expect("No parent directory");
        // Walk up to find Cargo.toml (project root)
        while !project_root.join("Cargo.toml").exists() {
            project_root = project_root.parent().expect("Reached filesystem root without finding project");
        }
        let test_disk_path = project_root.join("target/test_binaries.img");

        // Build test disk automatically - ALWAYS required
        eprintln!("[qemu-uefi] Building test disk image...");
        let build_status = Command::new("cargo")
            .args(&["run", "-p", "xtask", "--", "create-test-disk"])
            .current_dir(project_root)
            .status();

        match build_status {
            Ok(status) if status.success() => {
                eprintln!("[qemu-uefi] Test disk build complete");
            }
            Ok(status) => {
                eprintln!();
                eprintln!("╔══════════════════════════════════════════════════════════════╗");
                eprintln!("║  ❌ ERROR: TEST DISK BUILD FAILED                            ║");
                eprintln!("╠══════════════════════════════════════════════════════════════╣");
                eprintln!("║  Exit code: {:?}                                              ", status.code());
                eprintln!("║                                                              ║");
                eprintln!("║  Test disk is MANDATORY. There is NO fallback.              ║");
                eprintln!("║                                                              ║");
                eprintln!("║  To fix:                                                     ║");
                eprintln!("║    1. cargo run -p xtask -- create-test-disk                ║");
                eprintln!("║    2. Ensure userspace/tests/ binaries are compiled         ║");
                eprintln!("║                                                              ║");
                eprintln!("║  Exiting now to prevent silent test failures.               ║");
                eprintln!("╚══════════════════════════════════════════════════════════════╝");
                eprintln!();
                process::exit(1);
            }
            Err(e) => {
                eprintln!();
                eprintln!("╔══════════════════════════════════════════════════════════════╗");
                eprintln!("║  ❌ ERROR: TEST DISK BUILD COMMAND FAILED                    ║");
                eprintln!("╠══════════════════════════════════════════════════════════════╣");
                eprintln!("║  Error: {}                                                   ", e);
                eprintln!("║                                                              ║");
                eprintln!("║  Test disk is MANDATORY. There is NO fallback.              ║");
                eprintln!("║                                                              ║");
                eprintln!("║  To fix:                                                     ║");
                eprintln!("║    1. cargo run -p xtask -- create-test-disk                ║");
                eprintln!("║    2. Ensure userspace/tests/ binaries are compiled         ║");
                eprintln!("║                                                              ║");
                eprintln!("║  Exiting now to prevent silent test failures.               ║");
                eprintln!("╚══════════════════════════════════════════════════════════════╝");
                eprintln!();
                process::exit(1);
            }
        }

        // After successful build, verify disk exists
        if !test_disk_path.exists() {
            eprintln!();
            eprintln!("╔══════════════════════════════════════════════════════════════╗");
            eprintln!("║  ❌ ERROR: TEST DISK NOT FOUND AFTER BUILD                   ║");
            eprintln!("╠══════════════════════════════════════════════════════════════╣");
            eprintln!("║  Path: {}                                                   ", test_disk_path.display());
            eprintln!("║                                                              ║");
            eprintln!("║  Build reported success but disk image doesn't exist.       ║");
            eprintln!("║  This indicates a build system issue.                       ║");
            eprintln!("║                                                              ║");
            eprintln!("║  Exiting now to prevent silent test failures.               ║");
            eprintln!("╚══════════════════════════════════════════════════════════════╝");
            eprintln!();
            process::exit(1);
        }

        let disk_size = fs::metadata(&test_disk_path).map(|m| m.len()).unwrap_or(0);
        qemu.args([
            "-drive",
            &format!("if=none,id=testdisk,format=raw,file={}", test_disk_path.display()),
            "-device", "virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off",
        ]);
        eprintln!("[qemu-uefi] Test disk: {} ({} bytes) [virtio-blk device index 1]", test_disk_path.display(), disk_size);
    }
    // Improve CI capture and stability
    qemu.args([
        "-machine", "pc,accel=tcg",
        "-cpu", "qemu64",
        "-smp", "1",
        "-m", "512",
        "-nographic",
        "-boot", "strict=on",
        "-no-reboot",
        "-no-shutdown",
    ]);
    // QEMU monitor access for debugging (default: disabled)
    let monitor_mode = env::var("BREENIX_QEMU_MONITOR").unwrap_or_else(|_| "none".to_string());
    match monitor_mode.as_str() {
        "stdio" => {
            qemu.args(["-monitor", "stdio"]);
            eprintln!("[qemu-uefi] Monitor on stdio (WARNING: mixes with kernel output)");
        }
        "tcp" => {
            qemu.args(["-monitor", "tcp:127.0.0.1:4444,server,nowait"]);
            eprintln!("[qemu-uefi] Monitor on tcp:127.0.0.1:4444 (connect via: telnet localhost 4444)");
        }
        _ => {
            qemu.args(["-monitor", "none"]);
        }
    }
    // Deterministic guest-driven exit for CI via isa-debug-exit on port 0xF4
    qemu.args([
        "-device",
        "isa-debug-exit,iobase=0xf4,iosize=0x04",
    ]);
    // Optional debug log and firmware debug console capture
    if let Ok(log_path) = env::var("BREENIX_QEMU_LOG_PATH") {
        let debug_flags = env::var("BREENIX_QEMU_DEBUG_FLAGS")
            .unwrap_or_else(|_| "guest_errors".to_string());
        qemu.args(["-d", &debug_flags, "-D", &log_path]);
        eprintln!("[qemu-uefi] Debug log: {} (flags: {})", log_path, debug_flags);
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
    // Enable GDB debugging if BREENIX_GDB=1
    if env::var("BREENIX_GDB").ok().as_deref() == Some("1") {
        qemu.args(["-s", "-S"]);
        eprintln!("[qemu-uefi] ════════════════════════════════════════════════════════");
        eprintln!("[qemu-uefi] GDB server enabled on localhost:1234 (paused at startup)");
        eprintln!("[qemu-uefi] ════════════════════════════════════════════════════════");
        eprintln!("[qemu-uefi] Connect with:");
        eprintln!("[qemu-uefi]   gdb target/x86_64-breenix/release/kernel -ex 'target remote localhost:1234'");
        eprintln!("[qemu-uefi] Or use .gdbinit helper:");
        eprintln!("[qemu-uefi]   gdb target/x86_64-breenix/release/kernel");
        eprintln!("[qemu-uefi]   (gdb) breenix-connect");
        eprintln!("[qemu-uefi] ════════════════════════════════════════════════════════");
    }
    eprintln!("[qemu-uefi] Launching QEMU...");
    let exit_status = qemu.status().unwrap();
    process::exit(exit_status.code().unwrap_or(-1));
}