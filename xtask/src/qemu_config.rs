use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

/// Architecture-specific QEMU configuration.
#[derive(Debug, Clone)]
pub enum Arch {
    X86_64,
    Arm64,
}

impl Arch {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "x86_64" | "x86" => Ok(Arch::X86_64),
            "arm64" | "aarch64" => Ok(Arch::Arm64),
            _ => bail!("Unknown architecture: {}", s),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        }
    }

    pub fn is_arm64(&self) -> bool {
        matches!(self, Arch::Arm64)
    }
}

/// Configuration for a QEMU test run, encapsulating all platform differences
/// for building the kernel and launching QEMU.
pub struct QemuConfig {
    pub arch: Arch,
    pub features: Vec<String>,
    /// Serial output file paths, in the order they map to QEMU serial ports.
    /// For x86_64: COM1 (user output), COM2 (kernel logs).
    /// For ARM64: single serial port.
    pub serial_files: Vec<PathBuf>,
    /// The serial file to monitor for test markers / kernel log output.
    pub kernel_log_file: PathBuf,
    pub qmp_socket: Option<PathBuf>,
    pub extra_qemu_args: Vec<String>,
    pub ext2_disk: Option<PathBuf>,
}

impl QemuConfig {
    /// Build the kernel for this configuration.
    pub fn build(&self) -> Result<()> {
        match &self.arch {
            Arch::Arm64 => {
                let features = self.features.join(",");
                let status = Command::new("cargo")
                    .args([
                        "build",
                        "--release",
                        "--target",
                        "aarch64-breenix.json",
                        "-Z",
                        "build-std=core,alloc",
                        "-Z",
                        "build-std-features=compiler-builtins-mem",
                        "-p",
                        "kernel",
                        "--bin",
                        "kernel-aarch64",
                        "--features",
                        &features,
                    ])
                    .status()
                    .context("Failed to build ARM64 kernel")?;
                if !status.success() {
                    bail!("ARM64 kernel build failed");
                }
            }
            Arch::X86_64 => {
                let features = self.features.join(",");
                let status = Command::new("cargo")
                    .args([
                        "build",
                        "--release",
                        "-p",
                        "breenix",
                        "--features",
                        &features,
                        "--bin",
                        "qemu-uefi",
                    ])
                    .status()
                    .context("Failed to build x86_64 kernel")?;
                if !status.success() {
                    bail!("x86_64 kernel build failed");
                }
            }
        }
        Ok(())
    }

    /// Spawn QEMU with this configuration, returning the child process handle.
    pub fn spawn_qemu(&self) -> Result<std::process::Child> {
        match &self.arch {
            Arch::Arm64 => {
                let kernel_binary = "target/aarch64-breenix/release/kernel-aarch64";
                let mut args = vec![
                    "-M".to_string(),
                    "virt".to_string(),
                    "-cpu".to_string(),
                    "cortex-a72".to_string(),
                    "-m".to_string(),
                    "512".to_string(),
                    "-smp".to_string(),
                    "1".to_string(),
                    "-kernel".to_string(),
                    kernel_binary.to_string(),
                    "-display".to_string(),
                    "none".to_string(),
                    "-no-reboot".to_string(),
                ];

                // Add serial files
                for path in &self.serial_files {
                    args.push("-serial".to_string());
                    args.push(format!("file:{}", path.display()));
                }

                // Add ext2 disk if specified
                if let Some(disk) = &self.ext2_disk {
                    args.extend_from_slice(&[
                        "-device".to_string(),
                        "virtio-gpu-device".to_string(),
                        "-device".to_string(),
                        "virtio-keyboard-device".to_string(),
                        "-device".to_string(),
                        "virtio-blk-device,drive=ext2".to_string(),
                        "-drive".to_string(),
                        format!("if=none,id=ext2,format=raw,file={}", disk.display()),
                        "-device".to_string(),
                        "virtio-net-device,netdev=net0".to_string(),
                        "-netdev".to_string(),
                        "user,id=net0".to_string(),
                    ]);
                }

                // Add QMP socket if specified
                if let Some(qmp) = &self.qmp_socket {
                    args.push("-qmp".to_string());
                    args.push(format!("unix:{},server,nowait", qmp.display()));
                }

                // Add extra args
                args.extend(self.extra_qemu_args.iter().cloned());

                Command::new("qemu-system-aarch64")
                    .args(&args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .context("Failed to spawn qemu-system-aarch64")
            }
            Arch::X86_64 => {
                let features = self.features.join(",");
                let mut args = vec![
                    "run".to_string(),
                    "--release".to_string(),
                    "-p".to_string(),
                    "breenix".to_string(),
                    "--features".to_string(),
                    features,
                    "--bin".to_string(),
                    "qemu-uefi".to_string(),
                    "--".to_string(),
                ];

                // Add serial files
                for path in &self.serial_files {
                    args.push("-serial".to_string());
                    args.push(format!("file:{}", path.display()));
                }

                args.push("-display".to_string());
                args.push("none".to_string());

                // Add QMP socket if specified
                if let Some(qmp) = &self.qmp_socket {
                    args.push("-qmp".to_string());
                    args.push(format!("unix:{},server,nowait", qmp.display()));
                }

                // Add extra args
                args.extend(self.extra_qemu_args.iter().cloned());

                Command::new("cargo")
                    .args(&args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .context("Failed to spawn QEMU")
            }
        }
    }

    /// Create configuration for the kthread lifecycle test.
    pub fn for_kthread(arch: Arch) -> Self {
        let (serial_files, kernel_log_file) = match &arch {
            Arch::Arm64 => {
                let output = PathBuf::from("target/kthread_test_arm64_output.txt");
                (vec![output.clone()], output)
            }
            Arch::X86_64 => {
                let user = PathBuf::from("target/kthread_test_x86_64_user.txt");
                let kernel = PathBuf::from("target/kthread_test_x86_64_kernel.txt");
                (vec![user, kernel.clone()], kernel)
            }
        };

        QemuConfig {
            arch,
            features: vec!["kthread_test_only".to_string()],
            serial_files,
            kernel_log_file,
            qmp_socket: None,
            extra_qemu_args: vec![],
            ext2_disk: None,
        }
    }

    /// Create configuration for the boot-stages validator.
    pub fn for_boot_stages(arch: Arch) -> Self {
        match &arch {
            Arch::X86_64 => {
                let user = PathBuf::from("target/xtask_user_output.txt");
                let kernel = PathBuf::from("target/xtask_boot_stages_output.txt");
                QemuConfig {
                    arch,
                    features: vec!["testing".into(), "external_test_bins".into()],
                    serial_files: vec![user, kernel.clone()],
                    kernel_log_file: kernel,
                    qmp_socket: None,
                    extra_qemu_args: vec![],
                    ext2_disk: None,
                }
            }
            Arch::Arm64 => {
                let output = PathBuf::from("target/arm64_boot_stages_output.txt");
                QemuConfig {
                    arch,
                    features: vec!["testing".into()],
                    serial_files: vec![output.clone()],
                    kernel_log_file: output,
                    qmp_socket: None,
                    extra_qemu_args: vec![],
                    ext2_disk: Some(PathBuf::from("target/arm64_boot_stages_ext2.img")),
                }
            }
        }
    }

    /// Create configuration for the BTRT boot test.
    pub fn for_btrt(arch: Arch) -> Self {
        let serial_file = match &arch {
            Arch::Arm64 => PathBuf::from("target/btrt_arm64_output.txt"),
            Arch::X86_64 => PathBuf::from("target/btrt_x86_64_output.txt"),
        };
        let qmp_socket = PathBuf::from("/tmp/breenix-qmp.sock");

        let features = match &arch {
            Arch::Arm64 => vec!["btrt".to_string(), "testing".to_string()],
            Arch::X86_64 => vec![
                "btrt".to_string(),
                "testing".to_string(),
                "external_test_bins".to_string(),
            ],
        };

        let ext2_disk = match &arch {
            Arch::Arm64 => Some(PathBuf::from("target/btrt_arm64_ext2.img")),
            Arch::X86_64 => None,
        };

        QemuConfig {
            arch,
            features,
            serial_files: vec![serial_file.clone()],
            kernel_log_file: serial_file,
            qmp_socket: Some(qmp_socket),
            extra_qemu_args: vec![],
            ext2_disk,
        }
    }
}
