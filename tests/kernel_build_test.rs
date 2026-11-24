use std::env;
use std::path::PathBuf;
use std::process::Command;

/// Test that the kernel builds successfully with our custom target
#[test]
fn test_kernel_builds() {
    println!("Testing kernel build with custom target...");

    // Get the workspace root directory
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = PathBuf::from(&manifest_dir);
    let kernel_dir = workspace_root.join("kernel");
    let target_json = workspace_root.join("x86_64-breenix.json");

    // Build just the kernel crate with the custom target
    // We need to use nightly and build-std for custom targets
    let result = Command::new("cargo")
        .current_dir(&kernel_dir)
        .args(&[
            "build",
            "-Zbuild-std=core,compiler_builtins,alloc",
            "-Zbuild-std-features=compiler-builtins-mem",
            "--target",
            target_json.to_str().unwrap(),
            "--bin",
            "kernel",
        ])
        .output()
        .expect("Failed to run cargo build");

    if !result.status.success() {
        eprintln!("Build failed with stderr:");
        eprintln!("{}", String::from_utf8_lossy(&result.stderr));
    }

    assert!(result.status.success(), "Kernel build failed");
    println!("✅ Kernel builds successfully with custom target");
}

#[test]
fn test_kernel_builds_with_testing_feature() {
    println!("Testing kernel build with testing feature...");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = PathBuf::from(&manifest_dir);
    let kernel_dir = workspace_root.join("kernel");
    let target_json = workspace_root.join("x86_64-breenix.json");

    let result = Command::new("cargo")
        .current_dir(&kernel_dir)
        .args(&[
            "build",
            "-Zbuild-std=core,compiler_builtins,alloc",
            "-Zbuild-std-features=compiler-builtins-mem",
            "--target",
            target_json.to_str().unwrap(),
            "--features",
            "testing",
            "--bin",
            "kernel",
        ])
        .output()
        .expect("Failed to run cargo build");

    if !result.status.success() {
        eprintln!("Build failed with stderr:");
        eprintln!("{}", String::from_utf8_lossy(&result.stderr));
    }

    assert!(
        result.status.success(),
        "Kernel build with testing feature failed"
    );
    println!("✅ Kernel builds successfully with testing feature");
}

mod kernel_build {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TARGET: &str = "x86_64-unknown-none";
    const SELECTION: &str = "BREENIX_KERNEL_ASSEMBLER";
    const INPUTS: [(&str, &str); 4] = [
        ("src/syscall/entry.asm", "syscall_entry.o"),
        ("src/interrupts/timer_entry.asm", "timer_entry.o"),
        ("src/interrupts/breakpoint_entry.asm", "breakpoint_entry.o"),
        ("src/arch_impl/x86_64/ap_trampoline.asm", "ap_trampoline.o"),
    ];

    // Exercise the real build script and its four .asm inputs in Cargo. A small
    // linker entry and stubs for the Rust handlers keep the check independent
    // of how long rebuilding the rest of the kernel takes on a cold cache.
    struct Build {
        repo: PathBuf,
        scratch: PathBuf,
    }

    impl Build {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let scratch = std::env::temp_dir().join(format!(
                "breenix-kernel-build-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&scratch).unwrap();
            let build = Self { repo, scratch };
            let kernel = build.scratch.join("kernel");
            fs::create_dir(&kernel).unwrap();
            for name in [
                "build.rs",
                "stage_markers.rs",
                "assets",
                "verify_kepler_firmware.py",
                "linker.ld",
            ] {
                symlink(build.repo.join("kernel").join(name), kernel.join(name)).unwrap();
            }
            for source in INPUTS.map(|(source, _)| source).into_iter().chain([
                "src/arch_impl/aarch64/linker.ld",
                "src/gpu/gk208/gr/firmware.rs",
            ]) {
                let path = kernel.join(source);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                symlink(build.repo.join("kernel").join(source), path).unwrap();
            }
            for name in ["docs", "tools", "toolchain", "libs", "scripts"] {
                symlink(build.repo.join(name), build.scratch.join(name)).unwrap();
            }
            fs::write(
                kernel.join("Cargo.toml"),
                "[package]\nname = \"kernel\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\
                 [[bin]]\nname = \"kernel\"\npath = \"link_test.rs\"\n[profile.dev]\ndebug = 0\n",
            )
            .unwrap();
            fs::write(
                kernel.join("link_test.rs"),
                r#"
#![no_std]
#![no_main]
use core::arch::global_asm;
global_asm!(
    ".global _start",
    "_start:",
    "jmp syscall_entry",
    "jmp timer_interrupt_entry",
    "jmp breakpoint_entry",
    "lea rax, [rip + ap_trampoline_start]",
    ".global rust_syscall_handler, check_need_resched_and_switch, trace_iretq_to_ring3",
    ".global timer_interrupt_handler, log_timer_frame_from_userspace, send_timer_eoi",
    ".global dump_iret_frame_to_serial, rust_breakpoint_handler",
    "rust_syscall_handler:",
    "check_need_resched_and_switch:",
    "trace_iretq_to_ring3:",
    "timer_interrupt_handler:",
    "log_timer_frame_from_userspace:",
    "send_timer_eoi:",
    "dump_iret_frame_to_serial:",
    "rust_breakpoint_handler:",
    "ret",
);
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! { loop {} }
"#,
            )
            .unwrap();
            build
        }

        fn cargo(&self, selection: Option<&str>) -> Command {
            let mut cargo = if cfg!(target_os = "macos") {
                let mut command = Command::new("taskpolicy");
                command.args(["-b", "cargo"]);
                command
            } else {
                Command::new("cargo")
            };
            cargo
                .current_dir(&self.repo)
                .env_remove(SELECTION)
                .env_remove("RUSTFLAGS")
                .env("CARGO_BUILD_JOBS", "4")
                .env(
                    "CARGO_ENCODED_RUSTFLAGS",
                    "-Dwarnings\x1f-Clink-arg=--gc-sections",
                )
                .args(["build", "--offline", "--target", TARGET, "--manifest-path"])
                .arg(self.scratch.join("kernel/Cargo.toml"))
                .arg("--target-dir")
                .arg(self.scratch.join("target"));
            if let Some(value) = selection {
                cargo.env(SELECTION, value);
            }
            cargo
        }

        fn profile(&self) -> PathBuf {
            self.scratch.join("target").join(TARGET).join("debug")
        }

        fn out_dir(&self) -> PathBuf {
            let mut outputs = fs::read_dir(self.profile().join("build"))
                .unwrap()
                .map(|entry| entry.unwrap().path().join("out"))
                .filter(|path| path.is_dir());
            let dir = outputs.next().expect("kernel build script OUT_DIR");
            assert!(outputs.next().is_none());
            dir
        }
    }

    impl Drop for Build {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.scratch).unwrap();
        }
    }

    fn run(command: &mut Command) -> Output {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{command:?}\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        result
    }

    fn sha256(path: &Path) -> String {
        let result = run(Command::new("python3").args([
            "-c",
            "import hashlib, pathlib, sys; print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())",
        ]).arg(path));
        String::from_utf8(result.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn nasm_default_unchanged() {
        let build = Build::new();
        let path = std::env::var_os("PATH").unwrap();
        let nasm = std::env::split_paths(&path)
            .map(|dir| dir.join("nasm"))
            .find(|file| file.is_file())
            .expect("NASM on PATH");
        let recorder = build.scratch.join("nasm");
        let trace = build.scratch.join("nasm.argv");
        fs::write(
            &recorder,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$NASM_TRACE\"\nexec \"$REAL_NASM\" \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(recorder, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(
            std::iter::once(build.scratch.clone()).chain(std::env::split_paths(&path)),
        )
        .unwrap();
        run(build
            .cargo(None)
            .env("PATH", path)
            .env("NASM_TRACE", &trace)
            .env("REAL_NASM", &nasm));

        let out_dir = build.out_dir();
        let mut expected = String::new();
        for (source, object) in INPUTS {
            let source = build.scratch.join("kernel").join(source);
            let output = out_dir.join(object);
            expected.push_str(&format!(
                "-f\nelf64\n-o\n{}\n{}\n",
                output.display(),
                source.display()
            ));
            let reference = build.scratch.join(object);
            run(Command::new(&nasm)
                .args(["-f", "elf64", "-o"])
                .arg(&reference)
                .arg(&source));
            assert_eq!(
                fs::read(output).unwrap(),
                fs::read(reference).unwrap(),
                "{object}"
            );
        }
        assert_eq!(fs::read_to_string(trace).unwrap(), expected);
    }

    #[test]
    fn bas_digest_bound_to_build_host() {
        let build = Build::new();
        run(&mut build.cargo(Some("bas")));
        let out_dir = build.out_dir();
        let receipt = out_dir.join("kernel-assembly.record");
        let text = fs::read_to_string(&receipt).expect("linked kernel assembly record");
        let fields: BTreeMap<_, _> = text
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();

        let pin = fs::read_to_string(build.repo.join("toolchain/rust-toolchain.toml")).unwrap();
        let pin = pin
            .lines()
            .find_map(|line| line.strip_prefix("channel = \""))
            .unwrap();
        let rustc =
            run(Command::new("rustup").args(["run", pin.trim_end_matches('"'), "rustc", "-vV"]));
        let rustc = String::from_utf8(rustc.stdout).unwrap();
        let host = rustc
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap();
        let bas = out_dir.join("bas");
        let cargo_artifact = out_dir.join("bas-target").join(host).join("release/bas");
        assert_eq!(sha256(&bas), sha256(&cargo_artifact));
        assert_eq!(fields["cargo_package"], "bas");
        assert_eq!(fields["build_host"], host);
        assert_eq!(fields["rustc"], rustc.lines().next().unwrap());
        assert_eq!(fields["bas_sha256"], sha256(&bas));
        for (source, object) in INPUTS {
            assert_eq!(
                fields[source],
                sha256(&build.repo.join("kernel").join(source))
            );
            assert_eq!(fields[object], sha256(&out_dir.join(object)));
            let replay = build.scratch.join(object);
            run(Command::new(&bas)
                .args(["--target", "x86_64", "-o"])
                .arg(&replay)
                .arg(build.repo.join("kernel").join(source)));
            assert_eq!(
                fs::read(replay).unwrap(),
                fs::read(out_dir.join(object)).unwrap(),
                "{object}"
            );
        }
        let elf = build.profile().join("kernel");
        assert_eq!(&fs::read(&elf).unwrap()[..4], b"\x7fELF");
        assert_eq!(fields["kernel_elf_sha256"], sha256(&elf));

        let script_output = out_dir.parent().unwrap().join("output");
        let modified = |path: &Path| fs::metadata(path).unwrap().modified().unwrap();
        let before = (modified(&script_output), modified(&receipt), modified(&elf));
        run(&mut build.cargo(Some("bas")));
        assert_eq!(
            (modified(&script_output), modified(&receipt), modified(&elf)),
            before
        );
        assert_eq!(fs::read_to_string(receipt).unwrap(), text);

        let bad = build.cargo(Some("other")).output().unwrap();
        assert!(!bad.status.success());
        assert!(String::from_utf8_lossy(&bad.stderr).contains("BREENIX_KERNEL_ASSEMBLER"));
    }
}
