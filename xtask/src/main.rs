use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use structopt::StructOpt;

mod btrt_catalog;
mod btrt_parser;
mod boot_stages;
mod qemu_config;
mod qmp;
mod test_disk;
mod test_monitor;

/// Get the PID file path unique to this worktree.
/// Uses a hash of the current working directory to avoid conflicts between worktrees.
fn get_qemu_pid_file() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    let hash = hasher.finish();
    PathBuf::from(format!("/tmp/breenix-qemu-{:016x}.pid", hash))
}

/// Send a signal to this worktree's QEMU process.
/// Returns true if the process was found and signaled.
fn signal_worktree_qemu(signal: &str) -> bool {
    let pid_file = get_qemu_pid_file();
    if let Ok(pid_str) = fs::read_to_string(&pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            #[cfg(unix)]
            {
                // Check if process exists before signaling
                let exists = Command::new("kill")
                    .args(&["-0", &pid.to_string()])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if exists {
                    let _ = Command::new("kill")
                        .args(&[signal, &pid.to_string()])
                        .status();
                    return true;
                }
            }
        }
    }
    false
}

/// Kill any existing QEMU process that belongs to this worktree (SIGKILL).
/// Only kills the process if the PID file exists and the process is still running.
fn kill_worktree_qemu() {
    signal_worktree_qemu("-9");
    // Remove the stale PID file
    let _ = fs::remove_file(&get_qemu_pid_file());
}

/// Clean up a QEMU child process properly.
/// Kills the process, waits for it to exit, and removes the PID file.
fn cleanup_qemu_child(child: &mut std::process::Child) {
    // First try SIGTERM for graceful shutdown
    let _ = child.kill();

    // Wait with a short timeout
    for _ in 0..10 {
        match child.try_wait() {
            Ok(Some(_)) => {
                // Process has exited
                break;
            }
            Ok(None) => {
                // Still running, wait a bit
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }

    // Final wait to ensure it's reaped (non-blocking if already done)
    let _ = child.wait();

    // Clean up PID file
    let _ = fs::remove_file(get_qemu_pid_file());
}

/// Save the QEMU PID for this worktree.
fn save_qemu_pid(pid: u32) {
    let pid_file = get_qemu_pid_file();
    let _ = fs::write(&pid_file, pid.to_string());
}

fn build_std_test_binaries() -> Result<()> {
    println!("Building Rust std test binaries...\n");

    // Step 1: Build libbreenix-libc (produces libc.a)
    println!("  [1/2] Building libbreenix-libc...");
    let libc_dir = Path::new("libs/libbreenix-libc");

    if !libc_dir.exists() {
        println!("    Note: libs/libbreenix-libc not found, skipping std test binaries");
        return Ok(());
    }

    // Clear environment variables that might interfere with the standalone build
    // The rust-toolchain.toml in libbreenix-libc specifies the nightly version
    let status = Command::new("cargo")
        .args(&["build", "--release"])
        .current_dir(libc_dir)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_PKG_NAME")
        .env_remove("OUT_DIR")
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build for libbreenix-libc: {}", e))?;

    if !status.success() {
        bail!("Failed to build libbreenix-libc");
    }
    println!("    libbreenix-libc built successfully");

    // Step 2: Build userspace tests (produces hello_std_real)
    println!("  [2/2] Building userspace tests...");
    let tests_std_dir = Path::new("userspace/programs");

    if !tests_std_dir.exists() {
        println!("    Note: userspace/programs not found, skipping");
        return Ok(());
    }

    // The rust-toolchain.toml in tests specifies the nightly version
    // __CARGO_TESTS_ONLY_SRC_ROOT must point to the forked Rust library so that
    // -Z build-std compiles std from our patched sources (with target_os = "breenix")
    let rust_fork_library = std::env::current_dir()
        .unwrap_or_default()
        .join("rust-fork/library");
    let status = Command::new("cargo")
        .args(&["build", "--release"])
        .current_dir(tests_std_dir)
        .env("__CARGO_TESTS_ONLY_SRC_ROOT", &rust_fork_library)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_PKG_NAME")
        .env_remove("OUT_DIR")
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build for userspace tests: {}", e))?;

    if !status.success() {
        bail!("Failed to build userspace tests");
    }
    println!("    userspace tests built successfully");

    // Verify the binary exists
    let binary_path = tests_std_dir.join("target/x86_64-breenix/release/hello_std_real");
    if binary_path.exists() {
        println!("\n  hello_std_real binary ready at: {}", binary_path.display());
    } else {
        bail!(
            "Build succeeded but binary not found at: {}",
            binary_path.display()
        );
    }

    println!();
    Ok(())
}

/// Simple developer utility tasks.
#[derive(StructOpt)]
enum Cmd {
    /// Build Breenix and run the Ring-3 smoke test in QEMU.
    Ring3Smoke,
    /// Build Breenix and test ENOSYS syscall handling.
    Ring3Enosys,
    /// Boot kernel once and validate each boot stage sequentially.
    BootStages,
    /// Create test disk image containing all userspace test binaries.
    CreateTestDisk,
    /// Create ARM64 test disk image containing all ARM64 userspace test binaries.
    CreateTestDiskAarch64,
    /// Boot Breenix interactively with init_shell (serial console attached).
    Interactive,
    /// Run automated interactive shell tests (sends keyboard input via QEMU monitor).
    InteractiveTest,
    /// Run kthread stress test (100+ kthreads, rapid create/stop cycles).
    KthreadStress,
    /// Run focused DNS test only (faster iteration for network debugging).
    DnsTest,
    /// Boot ARM64 kernel and validate each boot stage sequentially.
    Arm64BootStages,
    /// Boot kernel with BTRT feature, extract results via QMP pmemsave.
    BootTestBtrt {
        /// Target architecture: x86_64 or arm64
        #[structopt(long, default_value = "x86_64")]
        arch: String,
    },
    /// Run kthread lifecycle test (x86_64 or arm64).
    KthreadTest {
        /// Target architecture: x86_64 or arm64
        #[structopt(long, default_value = "x86_64")]
        arch: String,
    },
    /// Parse a saved BTRT binary dump and display results.
    ParseBtrt {
        /// Path to the BTRT binary dump file.
        #[structopt(parse(from_os_str))]
        path: PathBuf,
        /// Output format: table, json, or ktap.
        #[structopt(long, default_value = "table")]
        format: String,
    },
}

fn main() -> Result<()> {
    match Cmd::from_args() {
        Cmd::Ring3Smoke => ring3_smoke(),
        Cmd::Ring3Enosys => ring3_enosys(),
        Cmd::BootStages => run_boot_stages(&qemu_config::Arch::X86_64),
        Cmd::CreateTestDisk => test_disk::create_test_disk(),
        Cmd::CreateTestDiskAarch64 => test_disk::create_test_disk_aarch64(),
        Cmd::Interactive => interactive(),
        Cmd::InteractiveTest => interactive_test(),
        Cmd::KthreadStress => kthread_stress(),
        Cmd::DnsTest => dns_test(),
        Cmd::Arm64BootStages => run_boot_stages(&qemu_config::Arch::Arm64),
        Cmd::BootTestBtrt { arch } => boot_test_btrt(&arch),
        Cmd::KthreadTest { arch } => kthread_test(&arch),
        Cmd::ParseBtrt { path, format } => parse_btrt_cmd(&path, &format),
    }
}

/// Focused DNS test - only checks DNS-related boot stages
/// Much faster iteration than full boot_stages when debugging network issues
fn dns_test() -> Result<()> {
    let stages = boot_stages::get_dns_stages();
    let total_stages = stages.len();

    println!("DNS Test - {} stages to check", total_stages);
    println!("=========================================\n");

    // Build std test binaries BEFORE creating the test disk
    build_std_test_binaries()?;

    // Create the test disk with all userspace binaries
    test_disk::create_test_disk()?;
    println!();

    // COM2 (log output) - this is where all test markers go
    let serial_output_file = "target/xtask_dns_test_output.txt";
    // COM1 (user output) - raw userspace output
    let user_output_file = "target/xtask_dns_user_output.txt";

    // Remove old output files
    let _ = fs::remove_file(serial_output_file);
    let _ = fs::remove_file(user_output_file);

    // Kill any existing QEMU for THIS worktree only
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Starting QEMU for DNS test...\n");

    // Start QEMU with dns_test_only feature (skips all other tests)
    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "dns_test_only",  // Uses minimal boot path
            "--bin",
            "qemu-uefi",
            "--",
            "-serial",
            &format!("file:{}", user_output_file),
            "-serial",
            &format!("file:{}", serial_output_file),
            "-display",
            "none",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn QEMU: {}", e))?;

    save_qemu_pid(child.id());

    // Wait for output file to be created
    let start = Instant::now();
    let file_creation_timeout = Duration::from_secs(60);

    while !std::path::Path::new(serial_output_file).exists() {
        if start.elapsed() > file_creation_timeout {
            cleanup_qemu_child(&mut child);
            bail!("Serial output file not created after {} seconds", file_creation_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Track which stages have passed
    let mut stages_passed = 0;
    let mut last_content_len = 0;
    let mut checked_stages: Vec<bool> = vec![false; total_stages];

    let test_start = Instant::now();
    // With dns_test_only feature, only essential boot + DNS test runs
    // Should complete in under 30 seconds
    let timeout = Duration::from_secs(60);

    loop {
        // Check timeout
        if test_start.elapsed() > timeout {
            cleanup_qemu_child(&mut child);
            println!("\n=========================================");
            println!("Result: {}/{} stages passed (TIMEOUT after {}s)", stages_passed, total_stages, timeout.as_secs());
            if stages_passed < total_stages {
                // Find first unpassed stage
                for (i, passed) in checked_stages.iter().enumerate() {
                    if !passed {
                        println!("\nFirst failed stage: [{}] {}", i + 1, stages[i].name);
                        println!("  Meaning: {}", stages[i].failure_meaning);
                        println!("  Check:   {}", stages[i].check_hint);
                        break;
                    }
                }
                bail!("DNS test incomplete - timeout");
            }
            break;
        }

        // Check if QEMU exited
        match child.try_wait() {
            Ok(Some(status)) => {
                // QEMU exited - do final check of both output files
                thread::sleep(Duration::from_millis(100));
                let kernel_content = fs::read_to_string(serial_output_file).unwrap_or_default();
                let user_content = fs::read_to_string(user_output_file).unwrap_or_default();
                for (i, stage) in stages.iter().enumerate() {
                    if !checked_stages[i] {
                        if kernel_content.contains(stage.marker) || user_content.contains(stage.marker) {
                            checked_stages[i] = true;
                            stages_passed += 1;
                            println!("[{}/{}] {}... PASS", i + 1, total_stages, stage.name);
                        }
                    }
                }
                println!("\n=========================================");
                if stages_passed == total_stages {
                    println!("Result: ALL {}/{} stages passed (total: {:.2}s)", stages_passed, total_stages, test_start.elapsed().as_secs_f64());
                    return Ok(());
                } else {
                    println!("Result: {}/{} stages passed (QEMU exit code: {:?})", stages_passed, total_stages, status.code());
                    for (i, passed) in checked_stages.iter().enumerate() {
                        if !passed {
                            println!("\nFirst failed stage: [{}] {}", i + 1, stages[i].name);
                            println!("  Meaning: {}", stages[i].failure_meaning);
                            println!("  Check:   {}", stages[i].check_hint);
                            break;
                        }
                    }
                    bail!("DNS test failed");
                }
            }
            Ok(None) => {
                // Still running
            }
            Err(e) => {
                bail!("Failed to check QEMU status: {}", e);
            }
        }

        // Read and check for markers from BOTH output files
        // - COM2 (kernel log): ARP and softirq markers
        // - COM1 (user output): DNS test markers (userspace prints to stdout -> COM1)
        let kernel_content = fs::read_to_string(serial_output_file).unwrap_or_default();
        let user_content = fs::read_to_string(user_output_file).unwrap_or_default();
        let combined_len = kernel_content.len() + user_content.len();

        if combined_len > last_content_len {
            last_content_len = combined_len;

            // Check all stages against both output sources
            for (i, stage) in stages.iter().enumerate() {
                if !checked_stages[i] {
                    if kernel_content.contains(stage.marker) || user_content.contains(stage.marker) {
                        checked_stages[i] = true;
                        stages_passed += 1;
                        println!("[{}/{}] {}... PASS", i + 1, total_stages, stage.name);
                    }
                }
            }

            // If all stages passed, we're done
            if stages_passed == total_stages {
                cleanup_qemu_child(&mut child);
                println!("\n=========================================");
                println!("Result: ALL {}/{} stages passed (total: {:.2}s)", stages_passed, total_stages, test_start.elapsed().as_secs_f64());
                return Ok(());
            }
        }

        thread::sleep(Duration::from_millis(100));
    }

    Ok(())
}

/// Prepare architecture-specific build artifacts for boot testing.
fn prepare_boot_test_artifacts(arch: &qemu_config::Arch) -> Result<()> {
    match arch {
        qemu_config::Arch::X86_64 => {
            // Build std test binaries BEFORE creating the test disk
            build_std_test_binaries()?;
            // Create the test disk with all userspace binaries
            test_disk::create_test_disk()?;
            println!();
            Ok(())
        }
        qemu_config::Arch::Arm64 => {
            // Skip if ext2 disk already exists (CI pre-builds userspace and disk separately)
            let ext2_img = Path::new("target/ext2-aarch64.img");
            if ext2_img.exists() {
                println!("ext2 disk image already exists at target/ext2-aarch64.img, skipping build steps");
            } else {
                println!("Building userspace test binaries for aarch64...");
                let userspace_dir = Path::new("userspace/programs");
                if userspace_dir.exists() {
                    let _ = fs::create_dir_all("userspace/programs/aarch64");
                    let status = Command::new("./build.sh")
                        .args(&["--arch", "aarch64"])
                        .current_dir(userspace_dir)
                        .status()
                        .map_err(|e| anyhow::anyhow!("Failed to run build.sh --arch aarch64: {}", e))?;
                    if !status.success() {
                        bail!("Failed to build userspace test binaries for aarch64");
                    }
                    println!("  aarch64 userspace binaries built successfully");
                } else {
                    println!("  Note: userspace/programs not found, skipping userspace build");
                }

                println!("\nCreating ext2 disk image for aarch64...");
                let create_disk_script = Path::new("scripts/create_ext2_disk.sh");
                if create_disk_script.exists() {
                    let use_sudo = cfg!(target_os = "linux") && std::env::var("CI").is_ok();
                    let status = if use_sudo {
                        Command::new("sudo")
                            .args(&["./scripts/create_ext2_disk.sh", "--arch", "aarch64"])
                            .status()
                    } else {
                        Command::new("./scripts/create_ext2_disk.sh")
                            .args(&["--arch", "aarch64"])
                            .status()
                    }
                    .map_err(|e| anyhow::anyhow!("Failed to run create_ext2_disk.sh: {}", e))?;

                    if !status.success() {
                        bail!("Failed to create ext2 disk image for aarch64");
                    }

                    if use_sudo {
                        let _ = Command::new("sudo")
                            .args(&["chown", "-R", &format!("{}:{}",
                                std::env::var("UID").unwrap_or_else(|_| "1000".to_string()),
                                std::env::var("GID").unwrap_or_else(|_| "1000".to_string()))])
                            .args(&["target/", "testdata/"])
                            .status();
                    }
                    println!("  ext2 disk image created successfully");
                } else {
                    bail!("scripts/create_ext2_disk.sh not found and no ext2 disk exists");
                }
            }

            if !ext2_img.exists() {
                bail!("ext2 disk image not found at target/ext2-aarch64.img");
            }

            // Create writable copy of ext2 disk (filesystem write tests modify it)
            let writable_ext2 = "target/arm64_boot_stages_ext2.img";
            fs::copy(ext2_img, writable_ext2)
                .map_err(|e| anyhow::anyhow!("Failed to create writable ext2 copy: {}", e))?;

            Ok(())
        }
    }
}

/// Unified boot stage runner for both x86_64 and ARM64 architectures.
///
/// Uses QemuConfig for kernel build/launch and a shared validation loop.
fn run_boot_stages(arch: &qemu_config::Arch) -> Result<()> {
    let stages = boot_stages::get_boot_stages(arch);
    let total_stages = stages.len();

    println!("{} Boot Stage Validator - {} stages to check", arch.label(), total_stages);
    println!("=========================================\n");

    // Prepare architecture-specific artifacts (userspace binaries, test disks)
    prepare_boot_test_artifacts(arch)?;

    // Build kernel and prepare QEMU config
    let config = qemu_config::QemuConfig::for_boot_stages(arch.clone());

    // Clean old serial files
    for f in &config.serial_files {
        let _ = fs::remove_file(f);
    }

    // Kill any existing QEMU for THIS worktree only (not other worktrees)
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Building {} kernel...\n", arch.label());
    config.build()?;

    println!("Starting QEMU...\n");
    let mut child = config.spawn_qemu()?;
    save_qemu_pid(child.id());

    // Shared validation loop
    validate_boot_stages(&stages, &mut child, &config.serial_files, arch.label())
}

/// Validate boot stages by monitoring serial output files for markers.
///
/// Reads from all provided serial files (x86_64 has 2: COM1+COM2, ARM64 has 1: UART),
/// concatenates their contents, and checks for stage markers.
fn validate_boot_stages(
    stages: &[boot_stages::BootStage],
    child: &mut std::process::Child,
    serial_files: &[PathBuf],
    arch_label: &str,
) -> Result<()> {
    let total_stages = stages.len();

    // Wait for first serial file to be created
    let start = Instant::now();
    let file_creation_timeout = Duration::from_secs(60);

    while !serial_files.iter().any(|f| f.exists()) {
        if start.elapsed() > file_creation_timeout {
            cleanup_qemu_child(child);
            bail!("Serial output file not created after {} seconds", file_creation_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Track which stages have passed
    let mut stages_passed = 0;
    let mut last_content_len = 0;
    let mut checked_stages: Vec<bool> = vec![false; total_stages];
    let mut stage_timings: Vec<Option<boot_stages::StageTiming>> = vec![None; total_stages];
    let mut stage_start_time = Instant::now();

    let test_start = Instant::now();
    // CI environments need more time due to virtualization overhead and resource contention
    let timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(480) // 8 minutes for CI
    } else {
        Duration::from_secs(300) // 5 minutes locally - allows time for QEMU serial buffer flush
    };
    // Note: QEMU's file-based serial output uses stdio buffering (~4KB). When tests complete
    // quickly, their markers may still be in QEMU's buffer when the validator reads the file.
    let stage_timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(90) // 90 seconds per stage in CI
    } else {
        Duration::from_secs(90) // 90 seconds per stage locally (matches CI for consistency)
    };
    let mut last_progress = Instant::now();

    // Print initial waiting message
    if let Some(stage) = stages.get(0) {
        print!("[{}/{}] {}...", 1, total_stages, stage.name);
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }

    while test_start.elapsed() < timeout {
        // Read all serial files and concatenate contents
        let mut combined_contents = String::new();
        for path in serial_files {
            if let Ok(mut file) = fs::File::open(path) {
                let mut contents_bytes = Vec::new();
                if file.read_to_end(&mut contents_bytes).is_ok() {
                    combined_contents.push_str(&String::from_utf8_lossy(&contents_bytes));
                }
            }
        }

        // Only process if content has changed
        if combined_contents.len() > last_content_len {
            last_content_len = combined_contents.len();
            let contents = &combined_contents;

            // Check each unchecked stage
            for (i, stage) in stages.iter().enumerate() {
                if !checked_stages[i] {
                    // Check if marker is found (support alternative patterns with |)
                    let found = if stage.marker.contains('|') {
                        stage.marker.split('|').any(|m| contents.contains(m))
                    } else {
                        contents.contains(stage.marker)
                    };

                    if found {
                        checked_stages[i] = true;
                        stages_passed += 1;
                        last_progress = Instant::now();

                        // Record timing for this stage
                        let duration = stage_start_time.elapsed();
                        stage_timings[i] = Some(boot_stages::StageTiming { duration });
                        stage_start_time = Instant::now();

                        // Format timing string
                        let time_str = if duration.as_secs() >= 1 {
                            format!("{:.2}s", duration.as_secs_f64())
                        } else {
                            format!("{}ms", duration.as_millis())
                        };

                        // Print result for this stage with timing
                        print!("\r[{}/{}] {}... ", i + 1, total_stages, stage.name);
                        for _ in 0..(50 - stage.name.len().min(50)) {
                            print!(" ");
                        }
                        println!("\r[{}/{}] {}... PASS ({})", i + 1, total_stages, stage.name, time_str);

                        // Print next stage we're waiting for
                        if i + 1 < total_stages {
                            if let Some(next_stage) = stages.get(i + 1) {
                                print!("[{}/{}] {}...", i + 2, total_stages, next_stage.name);
                                use std::io::Write;
                                let _ = std::io::stdout().flush();
                            }
                        }
                    }
                }
            }

            // Check for kernel panic
            if contents.contains("KERNEL PANIC") {
                println!("\r                                                              ");
                println!("\nKERNEL PANIC detected!\n");
                for line in contents.lines() {
                    if line.contains("KERNEL PANIC") || line.contains("panicked at") {
                        println!("  {}", line);
                    }
                }
                break;
            }

            // All stages passed?
            if stages_passed == total_stages {
                break;
            }
        }

        // Check for stage timeout (no new marker detected for stage_timeout duration).
        // Don't kill QEMU here - serial output may be buffered and the kernel might still
        // be making progress. Continue polling until the overall timeout expires.
        // This is critical for CI where nested virtualization causes QEMU serial buffers
        // to flush much less frequently than on native hardware.
        if last_progress.elapsed() > stage_timeout {
            // Check if QEMU is still running
            if let Ok(Some(_status)) = child.try_wait() {
                // QEMU exited - break to final scan
                break;
            }
            // QEMU still running - log the stall and reset timer to avoid repeated messages
            for (i, stage) in stages.iter().enumerate() {
                if !checked_stages[i] {
                    println!("\r[{}/{}] {}... (waiting, {}s elapsed)",
                        i + 1, total_stages, stage.name,
                        test_start.elapsed().as_secs());
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    break;
                }
            }
            last_progress = Instant::now();
        }

        thread::sleep(Duration::from_millis(50));
    }

    // Kill QEMU and wait for it to fully terminate
    cleanup_qemu_child(child);

    // Final scan of output files after QEMU terminates
    // This catches markers that were printed but not yet processed due to timing
    thread::sleep(Duration::from_millis(100));
    let mut combined_contents = String::new();
    for path in serial_files {
        if let Ok(mut file) = fs::File::open(path) {
            let mut contents_bytes = Vec::new();
            if file.read_to_end(&mut contents_bytes).is_ok() {
                combined_contents.push_str(&String::from_utf8_lossy(&contents_bytes));
            }
        }
    }
    if !combined_contents.is_empty() {
        let contents = &combined_contents;
        for (i, stage) in stages.iter().enumerate() {
            if !checked_stages[i] {
                let found = if stage.marker.contains('|') {
                    stage.marker.split('|').any(|m| contents.contains(m))
                } else {
                    contents.contains(stage.marker)
                };

                if found {
                    checked_stages[i] = true;
                    stages_passed += 1;
                    println!("[{}/{}] {}... PASS (found in final scan)", i + 1, total_stages, stage.name);
                }
            }
        }
    }

    println!();
    println!("=========================================");

    // Calculate total time
    let total_time: Duration = stage_timings.iter()
        .filter_map(|t| t.as_ref())
        .map(|t| t.duration)
        .sum();

    let total_str = if total_time.as_secs() >= 1 {
        format!("{:.2}s", total_time.as_secs_f64())
    } else {
        format!("{}ms", total_time.as_millis())
    };

    if stages_passed == total_stages {
        println!("Result: ALL {}/{} stages passed (total: {})", stages_passed, total_stages, total_str);
        Ok(())
    } else {
        // Find first failed stage
        for (i, stage) in stages.iter().enumerate() {
            if !checked_stages[i] {
                println!("Result: {}/{} stages passed", stages_passed, total_stages);
                println!();
                println!("First failed stage: [{}/{}] {}", i + 1, total_stages, stage.name);
                println!("  Meaning: {}", stage.failure_meaning);
                println!("  Check:   {}", stage.check_hint);
                break;
            }
        }

        bail!("{} boot stage validation incomplete", arch_label);
    }
}


/// Builds the kernel, boots it in QEMU, and asserts that the
/// hard-coded userspace program prints its greeting.
///
/// Uses dual serial ports: COM1 for user output, COM2 for kernel logs.
fn ring3_smoke() -> Result<()> {
    println!("Starting Ring-3 smoke test...");

    // COM2 (log output) - where test markers go via log::info!()
    let serial_output_file = "target/xtask_ring3_smoke_output.txt";
    // COM1 (user output) - raw userspace output
    let user_output_file = "target/xtask_ring3_user_output.txt";

    // Remove old output files if they exist
    let _ = fs::remove_file(serial_output_file);
    let _ = fs::remove_file(user_output_file);

    // Kill any existing QEMU for THIS worktree only (not other worktrees)
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Building and running kernel with testing features...");

    // Start QEMU with dual serial ports:
    // - COM1 (0x3F8) -> user output file
    // - COM2 (0x2F8) -> kernel log output file (where markers go)
    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins",
            "--bin",
            "qemu-uefi",
            "--",
            "-serial",
            &format!("file:{}", user_output_file),
            "-serial",
            &format!("file:{}", serial_output_file),
            "-display",
            "none",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn QEMU: {}", e))?;

    // Save the PID so other runs of this worktree can kill it if needed
    save_qemu_pid(child.id());

    println!("QEMU started, monitoring output...");

    // Wait for output file to be created (longer timeout for CI where build may be slower)
    let start = Instant::now();
    let file_creation_timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(300) // 5 minutes for CI
    } else {
        Duration::from_secs(30)  // 30 seconds locally
    };

    while !std::path::Path::new(serial_output_file).exists() {
        if start.elapsed() > file_creation_timeout {
            let _ = child.kill();
            bail!("Serial output file not created after {} seconds", file_creation_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(500));
    }

    // Monitor the output file for expected string
    let mut found = false;
    let test_start = Instant::now();
    let timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(60)  // 60 seconds for CI (kernel logs are verbose)
    } else {
        Duration::from_secs(30)  // 30 seconds locally
    };

    while test_start.elapsed() < timeout {
        if let Ok(mut file) = fs::File::open(serial_output_file) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                // Look for the RING3_SMOKE success marker or the completion marker
                if contents.contains("[ OK ] RING3_SMOKE: userspace executed + syscall path verified") ||
                   contents.contains("KERNEL_POST_TESTS_COMPLETE") {
                    found = true;
                    break;
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Kill QEMU
    cleanup_qemu_child(&mut child);

    // Print the output for debugging
    if let Ok(mut file) = fs::File::open(serial_output_file) {
        let mut contents = String::new();
        if file.read_to_string(&mut contents).is_ok() {
            println!("\n=== Kernel Output ===");
            for line in contents.lines().take(200) {
                println!("{}", line);
            }
            if contents.lines().count() > 200 {
                println!("... (truncated)");
            }
        }
    }

    if found {
        println!("\n  Ring-3 smoke test passed - userspace execution detected");
        Ok(())
    } else {
        bail!("\n  Ring-3 smoke test failed: no evidence of userspace execution");
    }
}

/// Builds the kernel, boots it in QEMU, and tests ENOSYS syscall handling.
///
/// Uses dual serial ports: COM1 for user output, COM2 for kernel logs.
fn ring3_enosys() -> Result<()> {
    println!("Starting Ring-3 ENOSYS test...");

    // COM2 (log output) - where test markers go via log::info!()
    let serial_output_file = "target/xtask_ring3_enosys_output.txt";
    // COM1 (user output) - raw userspace output
    let user_output_file = "target/xtask_ring3_enosys_user_output.txt";

    // Remove old output files if they exist
    let _ = fs::remove_file(serial_output_file);
    let _ = fs::remove_file(user_output_file);

    // Kill any existing QEMU for THIS worktree only (not other worktrees)
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Building and running kernel with testing features...");

    // Start QEMU with dual serial ports:
    // - COM1 (0x3F8) -> user output file
    // - COM2 (0x2F8) -> kernel log output file (where markers go)
    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins",
            "--bin",
            "qemu-uefi",
            "--",
            "-serial",
            &format!("file:{}", user_output_file),
            "-serial",
            &format!("file:{}", serial_output_file),
            "-display",
            "none",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn QEMU: {}", e))?;

    // Save the PID so other runs of this worktree can kill it if needed
    save_qemu_pid(child.id());

    println!("QEMU started, monitoring output...");

    // Wait for output file to be created
    let start = Instant::now();
    let file_creation_timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(300) // 5 minutes for CI
    } else {
        Duration::from_secs(30)  // 30 seconds locally
    };

    while !std::path::Path::new(serial_output_file).exists() {
        if start.elapsed() > file_creation_timeout {
            let _ = child.kill();
            bail!("Serial output file not created after {} seconds", file_creation_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(500));
    }

    // Monitor the output file for expected strings
    let mut found_enosys_ok = false;
    let mut found_enosys_fail = false;
    let mut _found_invalid_syscall = false;
    let test_start = Instant::now();
    let timeout = if std::env::var("CI").is_ok() {
        Duration::from_secs(60)  // 60 seconds for CI
    } else {
        Duration::from_secs(30)  // 30 seconds locally
    };

    while test_start.elapsed() < timeout {
        if let Ok(mut file) = fs::File::open(serial_output_file) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                // Look for ENOSYS test results
                // IMPORTANT: Must use specific prefix to avoid matching instructional messages
                // like "Should print 'ENOSYS OK'" which would cause false positives.
                if contents.contains("USERSPACE OUTPUT: ENOSYS OK") {
                    found_enosys_ok = true;
                    break;
                }

                // Also accept plain "ENOSYS OK\n" at start of line (actual userspace output)
                if contents.lines().any(|line| line.trim() == "ENOSYS OK") {
                    found_enosys_ok = true;
                    break;
                }

                if contents.contains("USERSPACE OUTPUT: ENOSYS FAIL") ||
                   contents.contains("ENOSYS FAIL") {
                    found_enosys_fail = true;
                    break;
                }

                // Also check for kernel warning about invalid syscall
                if contents.contains("Invalid syscall number: 999") ||
                   contents.contains("unknown syscall: 999") {
                    _found_invalid_syscall = true;
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Kill QEMU
    cleanup_qemu_child(&mut child);

    // Print the output for debugging
    if let Ok(mut file) = fs::File::open(serial_output_file) {
        let mut contents = String::new();
        if file.read_to_string(&mut contents).is_ok() {
            println!("\n=== Kernel Output ===");
            // Show lines containing ENOSYS or syscall-related messages
            for line in contents.lines() {
                if line.contains("ENOSYS") ||
                   line.contains("syscall") ||
                   line.contains("SYSCALL") ||
                   line.contains("Invalid") {
                    println!("{}", line);
                }
            }
        }
    }

    if found_enosys_fail {
        bail!("\n  ENOSYS test failed: syscall 999 did not return -38");
    } else if found_enosys_ok {
        println!("\n  ENOSYS test passed - syscall 999 correctly returned -38");
        Ok(())
    } else {
        bail!("\n  ENOSYS test failed: userspace did not report 'ENOSYS OK'.\n\
               This test requires:\n\
               1. Userspace process created successfully\n\
               2. Userspace executes syscall(999) from Ring 3\n\
               3. Userspace validates return value == -38\n\
               4. Userspace prints 'ENOSYS OK'");
    }
}

/// Boot Breenix interactively with init_shell and graphical PS/2 keyboard input.
///
/// Opens a QEMU graphical window where PS/2 keyboard input is captured and fed
/// to the init_shell via the keyboard interrupt handler.
///
/// Serial output goes to files for debugging:
/// - COM1 (0x3F8) -> target/serial_output.txt (user I/O from shell)
/// - COM2 (0x2F8) -> target/kernel.log (kernel debug logs)
fn interactive() -> Result<()> {
    println!("Building Breenix with interactive feature...");

    // Build with interactive feature
    let build_status = Command::new("cargo")
        .args(&[
            "build",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins,interactive",
            "--bin",
            "qemu-uefi",
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build: {}", e))?;

    if !build_status.success() {
        bail!("Build failed");
    }

    println!();
    println!("=== Breenix Interactive Mode ===");
    println!();
    println!("A QEMU window will open shortly.");
    println!();
    println!("Instructions:");
    println!("  - Type in the QEMU window (not this terminal)");
    println!("  - The PS/2 keyboard sends input to the Breenix shell");
    println!("  - Close the QEMU window or press the power button to exit");
    println!();
    println!("Serial logs (for debugging):");
    println!("  - Shell I/O:    target/serial_output.txt");
    println!("  - Kernel logs:  target/kernel.log");
    println!();
    println!("Tip: In another terminal, run:");
    println!("  tail -f target/serial_output.txt");
    println!("  tail -f target/kernel.log");
    println!();

    // Run QEMU with graphical display for PS/2 keyboard input:
    // - Cocoa display on macOS for keyboard input (PS/2 keyboard handler feeds stdin)
    // - COM1 (0x3F8) -> file:target/serial_output.txt for shell I/O
    // - COM2 (0x2F8) -> file:target/kernel.log for kernel logs
    // Set BREENIX_INTERACTIVE=1 to tell qemu-uefi runner we're in interactive mode
    let run_status = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins,interactive",
            "--bin",
            "qemu-uefi",
            "--",
            "-display",
            "cocoa",
            "-serial",
            "file:target/serial_output.txt",
            "-serial",
            "file:target/kernel.log",
        ])
        .env("BREENIX_INTERACTIVE", "1")
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run QEMU: {}", e))?;

    if run_status.success() {
        Ok(())
    } else {
        bail!("QEMU exited with error");
    }
}

/// Automated interactive shell tests using QEMU monitor for keyboard input
///
/// This test:
/// 1. Boots Breenix with init_shell and QEMU TCP monitor enabled
/// 2. Waits for the shell prompt
/// 3. Sends keyboard commands via QEMU monitor's `sendkey` command
/// 4. Verifies command output appears in serial logs
/// 5. Tests multiple commands to ensure shell continues working
fn interactive_test() -> Result<()> {
    println!("=== Interactive Shell Test ===");
    println!();
    println!("This test sends keyboard input via QEMU monitor to verify:");
    println!("  - Shell accepts keyboard input");
    println!("  - Commands produce expected output");
    println!("  - Shell continues working after multiple commands");
    println!();

    // Output files
    let user_output_file = "target/interactive_test_user.txt";
    let kernel_log_file = "target/interactive_test_kernel.txt";

    // Clean up old files
    let _ = fs::remove_file(user_output_file);
    let _ = fs::remove_file(kernel_log_file);

    // Kill any existing QEMU for THIS worktree only (not other worktrees)
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    println!("Building with interactive feature...");

    // Build first
    let build_status = Command::new("cargo")
        .args(&[
            "build",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins,interactive",
            "--bin",
            "qemu-uefi",
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build: {}", e))?;

    if !build_status.success() {
        bail!("Build failed");
    }

    println!("Starting QEMU with monitor enabled...");

    // Start QEMU with:
    // - TCP monitor on port 4444 for sending keyboard input
    // - Serial ports for capturing output
    // - No display (headless)
    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "testing,external_test_bins,interactive",
            "--bin",
            "qemu-uefi",
            "--",
            "-display",
            "none",
            "-serial",
            &format!("file:{}", user_output_file),
            "-serial",
            &format!("file:{}", kernel_log_file),
            "-monitor",
            "tcp:127.0.0.1:4444,server,nowait",
        ])
        .env("BREENIX_INTERACTIVE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn QEMU: {}", e))?;

    // Save the PID so other runs of this worktree can kill it if needed
    save_qemu_pid(child.id());

    // Helper to clean up on failure
    let cleanup = |child: &mut std::process::Child| {
        cleanup_qemu_child(child);
    };

    // Wait for output files to be created
    let start = Instant::now();
    let file_timeout = Duration::from_secs(60);
    while !std::path::Path::new(user_output_file).exists() {
        if start.elapsed() > file_timeout {
            cleanup(&mut child);
            bail!("Output file not created after {} seconds", file_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Wait for QEMU monitor to be ready
    println!("Waiting for QEMU monitor...");
    let monitor_start = Instant::now();
    let monitor_timeout = Duration::from_secs(10);
    let mut monitor: Option<TcpStream> = None;

    while monitor_start.elapsed() < monitor_timeout {
        if let Ok(stream) = TcpStream::connect("127.0.0.1:4444") {
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
            monitor = Some(stream);
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    let mut monitor = match monitor {
        Some(m) => m,
        None => {
            cleanup(&mut child);
            bail!("Could not connect to QEMU monitor on port 4444");
        }
    };

    // Read initial monitor prompt
    let mut buf = [0u8; 1024];
    let _ = monitor.read(&mut buf);

    println!("Connected to QEMU monitor");

    // Wait for shell prompt to appear in user output
    println!("Waiting for shell prompt...");
    let prompt_start = Instant::now();
    let prompt_timeout = Duration::from_secs(30);

    while prompt_start.elapsed() < prompt_timeout {
        if let Ok(contents) = fs::read_to_string(user_output_file) {
            if contents.contains("breenix>") {
                println!("Shell prompt detected!");
                break;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }

    // Helper function to send a string as keyboard input
    fn send_string(monitor: &mut TcpStream, s: &str) -> Result<()> {
        for c in s.chars() {
            let key = match c {
                'a'..='z' => c.to_string(),
                'A'..='Z' => format!("shift-{}", c.to_ascii_lowercase()),
                '0'..='9' => c.to_string(),
                ' ' => "spc".to_string(),
                '\n' => "ret".to_string(),
                '-' => "minus".to_string(),
                '_' => "shift-minus".to_string(),
                '.' => "dot".to_string(),
                '/' => "slash".to_string(),
                '\\' => "backslash".to_string(),
                ':' => "shift-semicolon".to_string(),
                ';' => "semicolon".to_string(),
                '=' => "equal".to_string(),
                '+' => "shift-equal".to_string(),
                '|' => "shift-backslash".to_string(),
                _ => continue, // Skip unsupported characters
            };
            let cmd = format!("sendkey {}\n", key);
            monitor.write_all(cmd.as_bytes())?;
            thread::sleep(Duration::from_millis(50)); // Small delay between keys
        }
        // Read any response from monitor
        let mut buf = [0u8; 256];
        let _ = monitor.read(&mut buf);
        Ok(())
    }

    // Helper to wait for string in output
    fn wait_for_output(file: &str, needle: &str, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Ok(contents) = fs::read_to_string(file) {
                if contents.contains(needle) {
                    return true;
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        false
    }

    // Track test results
    let mut tests_passed = 0;
    let mut tests_failed = 0;

    // Test 1: Run "help" command
    println!();
    println!("[Test 1] Sending 'help' command...");
    send_string(&mut monitor, "help\n")?;

    if wait_for_output(user_output_file, "Built-in commands:", Duration::from_secs(5)) {
        println!("  ✓ PASS: 'help' command produced expected output");
        tests_passed += 1;
    } else {
        println!("  ✗ FAIL: 'help' command did not produce expected output");
        tests_failed += 1;
    }

    // Give shell time to return to prompt
    thread::sleep(Duration::from_millis(500));

    // Test 2: Run "help" again to verify shell continues working
    println!();
    println!("[Test 2] Sending 'help' command again (testing subsequent commands)...");
    let output_len_before = fs::read_to_string(user_output_file).unwrap_or_default().len();
    send_string(&mut monitor, "help\n")?;

    thread::sleep(Duration::from_secs(2));
    let output_after = fs::read_to_string(user_output_file).unwrap_or_default();

    // Check if more output was produced (shell responded to second command)
    if output_after.len() > output_len_before + 100 {
        println!("  ✓ PASS: Shell responded to second 'help' command");
        tests_passed += 1;
    } else {
        println!("  ✗ FAIL: Shell did not respond to second command (this is the bug!)");
        tests_failed += 1;
    }

    // Test 3: Run "uptime" command
    println!();
    println!("[Test 3] Sending 'uptime' command...");
    send_string(&mut monitor, "uptime\n")?;

    // uptime prints "up N seconds" or similar
    if wait_for_output(user_output_file, "up ", Duration::from_secs(5)) {
        println!("  ✓ PASS: 'uptime' command produced expected output");
        tests_passed += 1;
    } else {
        println!("  ✗ FAIL: 'uptime' command did not produce expected output");
        tests_failed += 1;
    }

    // Test 4: Run "cat /hello.txt" to test argv passing
    println!();
    println!("[Test 4] Sending 'cat /hello.txt' command...");
    send_string(&mut monitor, "cat /hello.txt\n")?;
    thread::sleep(Duration::from_secs(3)); // Give time for cat to run and print debug

    let cat_output = fs::read_to_string(user_output_file).unwrap_or_default();
    // Check for cat debug output to understand what's happening
    if cat_output.contains("Hello from ext2") || cat_output.contains("Hello, World") {
        println!("  ✓ PASS: 'cat /hello.txt' displayed file contents");
        tests_passed += 1;
    } else if cat_output.contains("cat: missing file operand") {
        println!("  ✗ FAIL: cat didn't receive argv (argc < 2)");
        // Print debug info if available
        if let Some(debug_start) = cat_output.rfind("cat DEBUG:") {
            let debug_section: String = cat_output[debug_start..].lines().take(5).collect::<Vec<_>>().join("\n");
            println!("  Debug output: {}", debug_section);
        }
        tests_failed += 1;
    } else if cat_output.contains("cat DEBUG:") {
        // Debug output exists but file not found or other error
        println!("  ? cat debug output present, checking...");
        let debug_lines: Vec<&str> = cat_output.lines()
            .filter(|l| l.contains("cat DEBUG:") || l.contains("cat:"))
            .collect();
        for line in debug_lines.iter().take(10) {
            println!("    {}", line);
        }
        tests_failed += 1;
    } else {
        println!("  ? INCONCLUSIVE: Could not verify cat output");
        println!("  Last 5 lines of output:");
        for line in cat_output.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
            println!("    {}", line);
        }
    }

    // Test 5: Send Ctrl-C while at prompt (should just print ^C and continue)
    println!();
    println!("[Test 5] Sending Ctrl-C at prompt...");
    // Ctrl-C is sent as ctrl-c in QEMU
    monitor.write_all(b"sendkey ctrl-c\n")?;
    let _ = monitor.read(&mut buf);
    thread::sleep(Duration::from_secs(1));

    // Shell should still be responsive - send another command
    // Use a unique command "jobs" to detect if first char is lost (would show "Unknown command: obs")
    send_string(&mut monitor, "jobs\n")?;
    thread::sleep(Duration::from_secs(2));

    let ctrl_c_output = fs::read_to_string(user_output_file).unwrap_or_default();
    // Check for the specific bug: "Unknown command: obs" indicates first char was eaten
    if ctrl_c_output.contains("Unknown command: obs") {
        println!("  ✗ FAIL: First character after Ctrl-C was lost ('jobs' became 'obs')");
        tests_failed += 1;
    } else if ctrl_c_output.contains("No background jobs") || ctrl_c_output.contains("jobs") {
        println!("  ✓ PASS: Shell remained responsive after Ctrl-C");
        tests_passed += 1;
    } else {
        println!("  ? INCONCLUSIVE: Could not verify shell response after Ctrl-C");
        // Don't count as failure, just informational
    }

    // Test 6: Run spinner and Ctrl-C to interrupt it
    println!();
    println!("[Test 6] Running spinner and sending Ctrl-C to interrupt...");
    send_string(&mut monitor, "spinner\n")?;
    thread::sleep(Duration::from_secs(2)); // Let spinner start

    // Send Ctrl-C to interrupt
    monitor.write_all(b"sendkey ctrl-c\n")?;
    let _ = monitor.read(&mut buf);
    thread::sleep(Duration::from_secs(2));

    // Check if shell returned to prompt (should see another breenix> after the ^C)
    let spinner_output = fs::read_to_string(user_output_file).unwrap_or_default();
    let ctrl_c_count = spinner_output.matches("^C").count();
    if ctrl_c_count >= 2 && spinner_output.ends_with("breenix> ") || spinner_output.contains("breenix> \n") {
        println!("  ✓ PASS: Spinner interrupted and shell returned to prompt");
        tests_passed += 1;
    } else {
        // Check for page fault (the bug we fixed)
        if spinner_output.contains("Page fault") || spinner_output.contains("KERNEL PANIC") {
            println!("  ✗ FAIL: Kernel crashed (page fault or panic) when interrupting spinner");
            tests_failed += 1;
        } else {
            println!("  ? INCONCLUSIVE: Could not verify spinner interrupt behavior");
        }
    }

    // Clean up
    println!();
    println!("Cleaning up...");
    cleanup(&mut child);

    // Print final output for debugging
    println!();
    println!("=== User Output (last 50 lines) ===");
    if let Ok(contents) = fs::read_to_string(user_output_file) {
        for line in contents.lines().rev().take(50).collect::<Vec<_>>().into_iter().rev() {
            println!("{}", line);
        }
    }

    // Print summary
    println!();
    println!("=== Test Summary ===");
    println!("Passed: {}", tests_passed);
    println!("Failed: {}", tests_failed);

    if tests_failed > 0 {
        bail!("{} interactive test(s) failed", tests_failed);
    }

    println!();
    println!("All interactive tests passed!");
    Ok(())
}

/// Run the kthread stress test - 100+ kthreads with rapid create/stop cycles.
/// This is a dedicated test harness that ONLY runs the stress test and exits.
/// Run kthread lifecycle test for x86_64 or arm64.
/// Both architectures use the same markers and validation logic.
fn kthread_test(arch: &str) -> Result<()> {
    let arch = qemu_config::Arch::from_str(arch)?;
    let config = qemu_config::QemuConfig::for_kthread(arch);
    let arch_label = config.arch.label();

    println!("=== Kthread Lifecycle Test ({}) ===\n", arch_label);

    // Clean up previous serial output files
    for sf in &config.serial_files {
        let _ = fs::remove_file(sf);
    }

    // Kill any existing QEMU for this worktree
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    // Build
    println!("Building {} kernel with kthread_test_only feature...", arch_label);
    config.build()?;
    println!("  Build successful\n");

    // Launch QEMU
    println!("Starting QEMU...");
    let mut child = config.spawn_qemu()?;

    save_qemu_pid(child.id());

    // Monitor for completion using the unified TestMonitor
    let monitor = test_monitor::TestMonitor::new(
        config.kernel_log_file.clone(),
        "KTHREAD_TEST_ONLY_COMPLETE",
        Duration::from_secs(300),
    );
    let result = monitor.monitor(&mut child)?;

    // Kill QEMU regardless of outcome
    let _ = child.kill();
    let _ = child.wait();

    match result.outcome {
        test_monitor::MonitorOutcome::Success => {
            println!("\n=== Kthread Test Results ({}) [{:.1}s] ===\n", arch_label, result.duration.as_secs_f64());
            for line in result.serial_output.lines() {
                if line.contains("KTHREAD") {
                    println!("{}", line);
                }
            }
            println!("\n=== KTHREAD TEST PASSED ({}) ===\n", arch_label);
            Ok(())
        }
        test_monitor::MonitorOutcome::Panic => {
            println!("\n=== KTHREAD TEST FAILED (panic detected) ===\n");
            for line in result.serial_output.lines() {
                if line.contains("KTHREAD") || line.contains("panic") || line.contains("PANIC") {
                    println!("{}", line);
                }
            }
            bail!("Kthread test panicked ({})", arch_label);
        }
        test_monitor::MonitorOutcome::QemuExited(status) => {
            let status_str = status.map_or("unknown".to_string(), |c| c.to_string());
            println!("\n=== KTHREAD TEST FAILED (QEMU exited with {}) ===\n", status_str);
            for line in result.serial_output.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev() {
                println!("{}", line);
            }
            bail!("QEMU exited with {} before kthread tests completed ({})", status_str, arch_label);
        }
        test_monitor::MonitorOutcome::Timeout => {
            println!("\n=== KTHREAD TEST FAILED (timeout) ({}) ===\n", arch_label);
            println!("Last output:");
            for line in result.serial_output.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev() {
                println!("{}", line);
            }
            bail!("Kthread test timed out after {} seconds ({})", result.duration.as_secs(), arch_label);
        }
    }
}

/// In CI: Uses cargo run (like boot_stages) for proper OVMF handling.
/// Locally: Runs in Docker for clean isolation (no stray QEMU processes).
fn kthread_stress() -> Result<()> {
    let is_ci = std::env::var("CI").is_ok();

    if is_ci {
        kthread_stress_ci()
    } else {
        kthread_stress_docker()
    }
}

/// CI version: Uses cargo run like boot_stages (handles OVMF via ovmf-prebuilt crate)
fn kthread_stress_ci() -> Result<()> {
    println!("=== Kthread Stress Test (CI mode) ===\n");

    // COM1 (user output) and COM2 (kernel logs) - stress test markers go to COM2
    let user_output_file = "target/kthread_stress_user.txt";
    let serial_output_file = "target/kthread_stress_output.txt";
    let _ = fs::remove_file(user_output_file);
    let _ = fs::remove_file(serial_output_file);

    println!("Starting QEMU via cargo run...\n");

    let mut child = Command::new("cargo")
        .args(&[
            "run",
            "--release",
            "-p",
            "breenix",
            "--features",
            "kthread_stress_test",
            "--bin",
            "qemu-uefi",
            "--",
            "-serial",
            &format!("file:{}", user_output_file),  // COM1: user output
            "-serial",
            &format!("file:{}", serial_output_file), // COM2: kernel logs (test markers)
            "-display",
            "none",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn QEMU: {}", e))?;

    // Wait for output file to be created
    let start = Instant::now();
    let file_creation_timeout = Duration::from_secs(120);

    while !std::path::Path::new(serial_output_file).exists() {
        if start.elapsed() > file_creation_timeout {
            let _ = child.kill();
            bail!("Serial output file not created after {} seconds", file_creation_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Monitor for completion
    let timeout = Duration::from_secs(300); // 5 minutes for CI
    let mut success = false;
    let mut test_output = String::new();

    while start.elapsed() < timeout {
        if let Ok(contents) = fs::read_to_string(serial_output_file) {
            test_output = contents.clone();

            if contents.contains("KTHREAD_STRESS_TEST_COMPLETE") {
                success = true;
                break;
            }

            if contents.contains("panicked at") || contents.contains("PANIC:") {
                println!("\n=== STRESS TEST FAILED (panic detected) ===\n");
                for line in contents.lines() {
                    if line.contains("KTHREAD_STRESS") || line.contains("panic") || line.contains("PANIC") {
                        println!("{}", line);
                    }
                }
                let _ = child.kill();
                bail!("Kthread stress test panicked");
            }
        }
        thread::sleep(Duration::from_millis(500));
    }

    let _ = child.kill();

    if success {
        println!("\n=== Kthread Stress Test Results ===\n");
        for line in test_output.lines() {
            if line.contains("KTHREAD_STRESS") {
                println!("{}", line);
            }
        }
        println!("\n=== KTHREAD STRESS TEST PASSED ===\n");
        Ok(())
    } else {
        println!("\n=== STRESS TEST FAILED (timeout) ===\n");
        println!("Last output:");
        for line in test_output.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev() {
            println!("{}", line);
        }
        bail!("Kthread stress test timed out after {} seconds", timeout.as_secs());
    }
}

/// Local version: Uses Docker for clean QEMU isolation
fn kthread_stress_docker() -> Result<()> {
    println!("=== Kthread Stress Test (Docker) ===\n");

    // Step 0: Clean old build artifacts to ensure we use the fresh stress test build
    println!("Cleaning old build artifacts...");
    for entry in glob::glob("target/release/build/breenix-*").unwrap().filter_map(|p| p.ok()) {
        let _ = fs::remove_dir_all(&entry);
    }

    // Step 1: Build kernel with stress test feature
    println!("Building kernel with kthread_stress_test feature...");
    let build_status = Command::new("cargo")
        .args(&[
            "build",
            "--release",
            "--features",
            "kthread_stress_test",
            "--bin",
            "qemu-uefi",
            "-p",
            "breenix",
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run cargo build: {}", e))?;

    if !build_status.success() {
        bail!("Failed to build kernel with kthread_stress_test feature");
    }

    // Step 2: Find the built UEFI image
    let uefi_glob = "target/release/build/breenix-*/out/breenix-uefi.img";
    let uefi_img = glob::glob(uefi_glob)
        .map_err(|e| anyhow::anyhow!("Glob error: {}", e))?
        .filter_map(|p| p.ok())
        .next()
        .ok_or_else(|| anyhow::anyhow!("UEFI image not found at {}", uefi_glob))?;

    println!("Using UEFI image: {}", uefi_img.display());

    // Step 3: Build Docker image if needed
    let docker_dir = Path::new("docker/qemu");
    println!("Ensuring Docker image is built...");
    let docker_build = Command::new("docker")
        .args(&["build", "-t", "breenix-qemu", "."])
        .current_dir(docker_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if docker_build.is_err() || !docker_build.unwrap().success() {
        let _ = Command::new("docker")
            .args(&["build", "-t", "breenix-qemu", "."])
            .current_dir(docker_dir)
            .status();
    }

    // Step 4: Create temp directory for output
    let output_dir = PathBuf::from("/tmp/breenix_stress_test");
    let _ = fs::remove_dir_all(&output_dir);
    fs::create_dir_all(&output_dir)?;

    // Copy OVMF files
    fs::copy("target/ovmf/x64/code.fd", output_dir.join("OVMF_CODE.fd"))?;
    fs::copy("target/ovmf/x64/vars.fd", output_dir.join("OVMF_VARS.fd"))?;

    // Create empty output files
    fs::write(output_dir.join("serial_kernel.txt"), "")?;
    fs::write(output_dir.join("serial_user.txt"), "")?;

    // Step 5: Run QEMU in Docker with timeout
    println!("\nRunning kthread stress test in Docker (100+ kthreads)...\n");

    let uefi_img_abs = fs::canonicalize(&uefi_img)?;
    let test_binaries = fs::canonicalize("target/test_binaries.img").ok();
    let ext2_img = fs::canonicalize("target/ext2.img").ok();

    let mut docker_args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-v".to_string(),
        format!("{}:/breenix/breenix-uefi.img:ro", uefi_img_abs.display()),
        "-v".to_string(),
        format!("{}:/output", output_dir.display()),
    ];

    if let Some(ref tb) = test_binaries {
        docker_args.push("-v".to_string());
        docker_args.push(format!("{}:/breenix/test_binaries.img:ro", tb.display()));
    }
    if let Some(ref ext2) = ext2_img {
        docker_args.push("-v".to_string());
        docker_args.push(format!("{}:/breenix/ext2.img:ro", ext2.display()));
    }

    docker_args.extend([
        "breenix-qemu".to_string(),
        "qemu-system-x86_64".to_string(),
        "-pflash".to_string(), "/output/OVMF_CODE.fd".to_string(),
        "-pflash".to_string(), "/output/OVMF_VARS.fd".to_string(),
        "-drive".to_string(), "if=none,id=hd,format=raw,readonly=on,file=/breenix/breenix-uefi.img".to_string(),
        "-device".to_string(), "virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off".to_string(),
        "-machine".to_string(), "pc,accel=tcg".to_string(),
        "-cpu".to_string(), "qemu64".to_string(),
        "-smp".to_string(), "1".to_string(),
        "-m".to_string(), "512".to_string(),
        "-display".to_string(), "none".to_string(),
        "-no-reboot".to_string(),
        "-no-shutdown".to_string(),
        "-device".to_string(), "isa-debug-exit,iobase=0xf4,iosize=0x04".to_string(),
        "-serial".to_string(), "file:/output/serial_user.txt".to_string(),
        "-serial".to_string(), "file:/output/serial_kernel.txt".to_string(),
    ]);

    if test_binaries.is_some() {
        docker_args.extend([
            "-drive".to_string(), "if=none,id=testdisk,format=raw,readonly=on,file=/breenix/test_binaries.img".to_string(),
            "-device".to_string(), "virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off".to_string(),
        ]);
    }
    if ext2_img.is_some() {
        docker_args.extend([
            "-drive".to_string(), "if=none,id=ext2disk,format=raw,readonly=on,file=/breenix/ext2.img".to_string(),
            "-device".to_string(), "virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off".to_string(),
        ]);
    }

    let mut docker_child = Command::new("docker")
        .args(&docker_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn Docker: {}", e))?;

    // Step 6: Monitor output file for completion
    let start = Instant::now();
    let timeout = Duration::from_secs(180);
    let kernel_log = output_dir.join("serial_kernel.txt");
    let mut success = false;
    let mut test_output = String::new();

    while start.elapsed() < timeout {
        if let Ok(contents) = fs::read_to_string(&kernel_log) {
            test_output = contents.clone();

            if contents.contains("KTHREAD_STRESS_TEST_COMPLETE") {
                success = true;
                break;
            }

            if contents.contains("panicked at") || contents.contains("PANIC:") {
                println!("\n=== STRESS TEST FAILED (panic detected) ===\n");
                for line in contents.lines() {
                    if line.contains("KTHREAD_STRESS") || line.contains("panic") || line.contains("PANIC") {
                        println!("{}", line);
                    }
                }
                let _ = docker_child.kill();
                let _ = Command::new("sh")
                    .args(&["-c", "docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true"])
                    .status();
                bail!("Kthread stress test panicked");
            }
        }
        thread::sleep(Duration::from_millis(500));
    }

    let _ = docker_child.kill();
    let _ = Command::new("sh")
        .args(&["-c", "docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true"])
        .status();

    if success {
        println!("\n=== Kthread Stress Test Results ===\n");
        for line in test_output.lines() {
            if line.contains("KTHREAD_STRESS") {
                println!("{}", line);
            }
        }
        println!("\n=== KTHREAD STRESS TEST PASSED ===\n");
        Ok(())
    } else {
        println!("\n=== STRESS TEST FAILED (timeout) ===\n");
        println!("Last output:");
        for line in test_output.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev() {
            println!("{}", line);
        }
        bail!("Kthread stress test timed out after {} seconds", timeout.as_secs());
    }
}

// =============================================================================
// BTRT Boot Test Command
// =============================================================================

/// Boot the kernel with BTRT feature, wait for BTRT_READY sentinel,
/// extract the BTRT via QMP pmemsave, parse and report results.
fn parse_btrt_cmd(path: &Path, format: &str) -> Result<()> {
    let results = btrt_parser::parse_file(path)?;

    match format {
        "json" => btrt_parser::print_json(&results),
        "ktap" => btrt_parser::print_ktap(&results),
        "table" => btrt_parser::print_detailed(&results),
        _ => bail!("Unknown format '{}'. Use: table, json, or ktap", format),
    }

    if btrt_parser::all_passed(&results) {
        Ok(())
    } else {
        bail!("BTRT: {} tests failed", results.header.tests_failed)
    }
}

fn boot_test_btrt(arch: &str) -> Result<()> {
    println!("BTRT Boot Test - arch: {}", arch);
    println!("========================================\n");

    let arch = qemu_config::Arch::from_str(arch)?;
    let config = qemu_config::QemuConfig::for_btrt(arch);

    let btrt_bin = "/tmp/btrt-results.bin";

    // Clean up previous artifacts
    if let Some(ref qmp) = config.qmp_socket {
        let _ = fs::remove_file(qmp);
    }
    let _ = fs::remove_file(btrt_bin);
    for sf in &config.serial_files {
        let _ = fs::remove_file(sf);
    }

    // Kill any existing QEMU for this worktree
    kill_worktree_qemu();
    thread::sleep(Duration::from_millis(500));

    // Architecture-specific pre-build steps
    if config.arch.is_arm64() {
        println!("Building ARM64 kernel with btrt feature...");

        // Build ARM64 userspace and ext2 disk image if not already present
        let ext2_img = Path::new("target/ext2-aarch64.img");
        if !ext2_img.exists() {
            // Build ARM64 userspace binaries
            println!("  Building ARM64 userspace binaries...");
            let userspace_dir = Path::new("userspace/programs");
            if userspace_dir.exists() {
                let _ = fs::create_dir_all("userspace/programs/aarch64");
                let status = Command::new("./build.sh")
                    .args(["--arch", "aarch64"])
                    .current_dir(userspace_dir)
                    .status()
                    .context("Failed to run build.sh --arch aarch64")?;
                if !status.success() {
                    bail!("Failed to build ARM64 userspace binaries");
                }
                println!("  ARM64 userspace binaries built successfully");
            } else {
                bail!("userspace/programs directory not found");
            }

            // Create ARM64 ext2 disk image
            println!("  Creating ARM64 ext2 disk image...");
            let create_disk_script = Path::new("scripts/create_ext2_disk.sh");
            if create_disk_script.exists() {
                let use_sudo = cfg!(target_os = "linux") && std::env::var("CI").is_ok();
                let status = if use_sudo {
                    Command::new("sudo")
                        .args(["./scripts/create_ext2_disk.sh", "--arch", "aarch64"])
                        .status()
                } else {
                    Command::new("./scripts/create_ext2_disk.sh")
                        .args(["--arch", "aarch64"])
                        .status()
                }
                .context("Failed to run create_ext2_disk.sh --arch aarch64")?;
                if !status.success() {
                    bail!("Failed to create ARM64 ext2 disk image");
                }
                if use_sudo {
                    let _ = Command::new("sudo")
                        .args(["chown", "-R", &format!("{}:{}",
                            std::env::var("UID").unwrap_or_else(|_| "1000".to_string()),
                            std::env::var("GID").unwrap_or_else(|_| "1000".to_string()))])
                        .args(["target/", "testdata/"])
                        .status();
                }
                println!("  ARM64 ext2 disk image created successfully");
            } else {
                bail!("scripts/create_ext2_disk.sh not found and no ext2 disk exists");
            }
        }

        // Copy ext2 image to writable location before QEMU launch
        let _ = fs::copy("target/ext2-aarch64.img", "target/btrt_arm64_ext2.img");
    } else {
        println!("Building x86_64 kernel with btrt feature...");
        build_std_test_binaries()?;
        test_disk::create_test_disk()?;
    }

    // Build kernel
    config.build()?;
    println!("  Build successful\n");

    // Launch QEMU with QMP socket
    println!("Starting QEMU with QMP socket...");
    let mut child = config.spawn_qemu()?;

    save_qemu_pid(child.id());

    // Wait for serial output file
    let serial_output_file = &config.kernel_log_file;
    let start = Instant::now();
    let file_timeout = Duration::from_secs(60);
    while !serial_output_file.exists() {
        if start.elapsed() > file_timeout {
            cleanup_qemu_child(&mut child);
            bail!("Serial output file not created after {} seconds", file_timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }

    // Monitor serial for BTRT_READY sentinel and [btrt] phys address line
    println!("Waiting for BTRT_READY sentinel...\n");
    let test_start = Instant::now();
    let test_timeout = Duration::from_secs(300);
    // After finding the BTRT address, wait up to this long for BTRT_READY
    // before extracting anyway (handles stuck test processes).
    let addr_grace_period = Duration::from_secs(120);
    let mut btrt_phys_addr: Option<u64> = None;
    let mut btrt_size: Option<u64> = None;
    let mut btrt_ready = false;
    let mut btrt_addr_found_time: Option<Instant> = None;
    let mut last_len = 0;

    while test_start.elapsed() < test_timeout {
        if let Ok(contents) = fs::read_to_string(serial_output_file) {
            if contents.len() > last_len {
                // Parse any new content for [btrt] address line
                let new_content = &contents[last_len..];
                for line in new_content.lines() {
                    if line.contains("[btrt]") && line.contains("phys") {
                        // Parse: [btrt] Boot Test Result Table at phys 0xADDR (SIZE bytes)
                        if let Some(addr_str) = line.split("phys ").nth(1) {
                            if let Some(addr_hex) = addr_str.split_whitespace().next() {
                                let cleaned = addr_hex.trim_start_matches("0x").trim_start_matches("0X");
                                if let Ok(addr) = u64::from_str_radix(cleaned, 16) {
                                    btrt_phys_addr = Some(addr);
                                    if btrt_addr_found_time.is_none() {
                                        btrt_addr_found_time = Some(Instant::now());
                                    }
                                }
                            }
                        }
                        if let Some(size_part) = line.split('(').nth(1) {
                            if let Some(size_str) = size_part.split_whitespace().next() {
                                btrt_size = size_str.parse().ok();
                            }
                        }
                    }
                }
                last_len = contents.len();
            }

            if contents.contains("===BTRT_READY===") {
                btrt_ready = true;
                break;
            }

            // Grace period fallback: if we found the BTRT address more than
            // addr_grace_period ago and still no BTRT_READY, some test process
            // is stuck. Extract the BTRT data anyway -- it accurately reflects
            // which tests passed/failed/are still running.
            if let Some(found_time) = btrt_addr_found_time {
                if found_time.elapsed() > addr_grace_period {
                    eprintln!("WARNING: BTRT address found {}s ago but BTRT_READY not received",
                              found_time.elapsed().as_secs());
                    eprintln!("         Proceeding with BTRT extraction (some test processes may be stuck)");
                    btrt_ready = true;
                    break;
                }
            }
        }

        // Check if QEMU has exited
        if let Ok(Some(status)) = child.try_wait() {
            // QEMU exited - check if we got BTRT_READY before it died
            if let Ok(contents) = fs::read_to_string(serial_output_file) {
                if contents.contains("===BTRT_READY===") {
                    btrt_ready = true;
                    break;
                }
            }
            bail!("QEMU exited with status {} before BTRT_READY", status);
        }

        thread::sleep(Duration::from_millis(200));
    }

    if !btrt_ready {
        cleanup_qemu_child(&mut child);
        bail!("Timed out waiting for BTRT_READY sentinel");
    }

    let phys_addr = match btrt_phys_addr {
        Some(addr) => addr,
        None => {
            cleanup_qemu_child(&mut child);
            bail!("BTRT_READY received but [btrt] physical address not found in serial output");
        }
    };

    let size = btrt_size.unwrap_or(40960); // Default to 40KB if not parsed
    println!("BTRT ready at phys {:#018x} ({} bytes)", phys_addr, size);

    // Connect to QMP and extract BTRT
    println!("Extracting BTRT via QMP pmemsave...");

    // Small delay to ensure QMP socket is ready
    thread::sleep(Duration::from_millis(500));

    let qmp_sock = config
        .qmp_socket
        .as_ref()
        .expect("QemuConfig::for_btrt always sets qmp_socket");

    match qmp::QmpClient::connect(qmp_sock) {
        Ok(mut qmp_client) => {
            qmp_client.stop()?;
            qmp_client.pmemsave(phys_addr, size, btrt_bin)?;

            // Small delay for pmemsave to complete the file write
            thread::sleep(Duration::from_millis(200));

            qmp_client.quit()?;
        }
        Err(e) => {
            // QMP connection failed -- fall back to parsing KTAP from serial output
            eprintln!("QMP connection failed: {} (falling back to serial KTAP)", e);
            cleanup_qemu_child(&mut child);

            // Print the serial output which contains KTAP lines
            if let Ok(contents) = fs::read_to_string(serial_output_file) {
                println!("\n--- Serial output (KTAP lines) ---");
                for line in contents.lines() {
                    if line.starts_with("ok ")
                        || line.starts_with("not ok ")
                        || line.starts_with("KTAP ")
                        || line.starts_with("1..")
                        || line.starts_with("# ")
                        || line.starts_with("===")
                    {
                        println!("{}", line);
                    }
                }
            }
            println!("\nNote: QMP extraction failed; results shown from serial KTAP output only.");
            return Ok(());
        }
    }

    // Wait for QEMU to actually exit
    let _ = child.wait();

    // Parse the BTRT binary blob
    if !Path::new(btrt_bin).exists() {
        bail!("BTRT binary file not written by QMP pmemsave");
    }

    let results = btrt_parser::parse_file(btrt_bin)?;

    // Print results
    btrt_parser::print_summary(&results);
    println!();
    btrt_parser::print_ktap(&results);

    // Validate shell prompt appeared (confirms full boot-to-shell path)
    if let Ok(contents) = fs::read_to_string(serial_output_file) {
        if contents.contains("breenix>") {
            println!("Shell prompt validated: breenix> found in serial output");
        } else {
            eprintln!("WARNING: Shell prompt 'breenix>' not found in serial output");
        }
    }

    if btrt_parser::all_passed(&results) {
        println!("\n=== BTRT BOOT TEST PASSED ===\n");
        Ok(())
    } else {
        bail!(
            "BTRT boot test failed: {} of {} tests failed",
            results.header.tests_failed,
            results.header.tests_completed
        );
    }
}
