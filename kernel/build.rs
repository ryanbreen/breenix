use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "stage_markers.rs"]
mod stage_markers;

#[path = "../libs/libpixcheck/src/sha256.rs"]
mod sha256;

const ASSEMBLER: &str = "BREENIX_KERNEL_ASSEMBLER";
const ASSEMBLY_RECORD: &str = "kernel-assembly.record";
const LINK_RECORD_ARG: &str = "--breenix-bas-record=";
const AARCH64_ASSEMBLY_DIR: &str = "src/arch_impl/aarch64";
const AARCH64_GUARD: &str = "resume_pc_guard.inc";
const AARCH64_ASSEMBLY: [(&str, &str, &str); 2] = [
    ("boot.S", "boot.s", "boot.o"),
    ("syscall_entry.S", "syscall_entry.s", "syscall_entry.o"),
];
const X86_ASSEMBLY: [(&str, &str); 4] = [
    ("src/syscall/entry.asm", "syscall_entry.o"),
    ("src/interrupts/timer_entry.asm", "timer_entry.o"),
    ("src/interrupts/breakpoint_entry.asm", "breakpoint_entry.o"),
    ("src/arch_impl/x86_64/ap_trampoline.asm", "ap_trampoline.o"),
];

fn digest(path: &Path) -> String {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    digest_bytes(&bytes)
}

fn digest_bytes(bytes: &[u8]) -> String {
    sha256::sha256(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").unwrap();
            hex
        })
}

fn remove_record(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        assert_eq!(
            error.kind(),
            io::ErrorKind::NotFound,
            "remove {}: {error}",
            path.display()
        );
    }
}

// Cargo supplies the final linker output path only when rustc invokes the
// linker. Publish the record after that link succeeds.
fn link_kernel(args: Vec<OsString>) {
    let marker = args.iter().position(|arg| {
        arg.to_str()
            .is_some_and(|arg| arg.starts_with(LINK_RECORD_ARG))
    });
    let mut linker = Command::new("rust-lld");

    let Some(marker) = marker else {
        let status = linker.args(&args).status().expect("run rust-lld");
        process::exit(status.code().unwrap_or(1));
    };
    let record = Path::new(
        args[marker]
            .to_str()
            .unwrap()
            .strip_prefix(LINK_RECORD_ARG)
            .unwrap(),
    );
    let elf = args
        .windows(2)
        .find_map(|pair| (pair[0] == "-o").then(|| Path::new(&pair[1])))
        .expect("rust-lld output argument");
    remove_record(record);
    let status = linker
        .args(&args[..marker])
        .args(&args[marker + 1..])
        .status()
        .expect("link kernel with rust-lld");
    if !status.success() {
        process::exit(status.code().unwrap_or(1));
    }

    let mut text =
        fs::read_to_string(record.with_extension("inputs")).expect("read kernel assembly inputs");
    writeln!(text, "kernel_elf_sha256={}", digest(elf)).unwrap();
    let pending = record.with_extension("tmp");
    fs::write(&pending, text).expect("write kernel assembly record");
    fs::rename(pending, record).expect("publish kernel assembly record");
}

fn host_tool(pin: &str, tool: &str, out_dir: &Path) -> Command {
    let mut command = Command::new("rustup");
    command.args(["run", pin, tool]).current_dir(out_dir);
    for (key, _) in env::vars_os() {
        let name = key.as_encoded_bytes();
        if name.starts_with(b"CARGO_") || name.starts_with(b"__CARGO_") {
            command.env_remove(key);
        }
    }
    for key in [
        "CARGO",
        "RUSTFLAGS",
        "ENCODED_RUSTFLAGS",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUST_TARGET_PATH",
        "RUSTUP_TOOLCHAIN",
        "MAKEFLAGS",
        "MFLAGS",
    ] {
        command.env_remove(key);
    }
    command.env("CARGO_HOME", out_dir.join("bas-cargo-home"));
    command
}

fn build_host_bas(toolchain: &Path, out_dir: &Path) -> (PathBuf, String) {
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "bas/Cargo.toml",
        "bas/src",
        "bobj/Cargo.toml",
        "bobj/src",
    ] {
        println!("cargo:rerun-if-changed={}", toolchain.join(path).display());
    }
    let pin =
        fs::read_to_string(toolchain.join("rust-toolchain.toml")).expect("read toolchain pin");
    let pin = pin
        .lines()
        .find_map(|line| line.trim().strip_prefix("channel = \"")?.strip_suffix('"'))
        .expect("toolchain pin must specify a channel");
    let rustc = host_tool(pin, "rustc", out_dir)
        .arg("-vV")
        .output()
        .expect("run pinned rustc");
    assert!(
        rustc.status.success(),
        "run pinned rustc: {}",
        String::from_utf8_lossy(&rustc.stderr)
    );
    let identity = String::from_utf8(rustc.stdout).expect("rustc version is UTF-8");
    let version = identity.lines().next().expect("rustc version");
    let host = identity
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("rustc host triple");
    assert_eq!(
        Some(OsStr::new(host)),
        env::var_os("HOST").as_deref(),
        "pinned bas toolchain must run on Cargo's host"
    );

    let target_dir = out_dir.join("bas-target");
    // The outer Cargo may hold its package-cache lock for the entire build
    // (ecedaa3e). This workspace uses only path packages and its own cache,
    // target directory and job count, so the host build can run offline.
    let status = host_tool(pin, "cargo", out_dir)
        .args([
            "build",
            "--offline",
            "--locked",
            "--release",
            "--jobs",
            "4",
            "--target",
            host,
        ])
        .arg("--target-dir")
        .arg(&target_dir)
        .arg("--manifest-path")
        .arg(toolchain.join("Cargo.toml"))
        .args(["-p", "bas", "--bin", "bas"])
        .status()
        .expect("run pinned Cargo for host bas");
    assert!(status.success(), "build host bas");

    let bas = out_dir.join("bas");
    let pending = bas.with_extension("tmp");
    fs::copy(target_dir.join(host).join("release/bas"), &pending).expect("stage host bas");
    // Cargo checks watched inputs against the start of the build script.
    // Stage the generated executable with an older mtime so our own write
    // leaves it fresh. Replacing or editing it later still invalidates it.
    fs::File::options()
        .write(true)
        .open(&pending)
        .expect("open staged bas")
        .set_modified(UNIX_EPOCH)
        .expect("timestamp staged bas");
    fs::rename(pending, &bas).expect("publish host bas");
    println!("cargo:rerun-if-changed={}", bas.display());

    let record = format!(
        "cargo_package=bas\nbuild_host={host}\nrustc={version}\nbas_sha256={}\n",
        digest(&bas),
    );
    (bas, record)
}

fn record_kernel_link(record_path: &Path, bin: &str, inputs: String) {
    fs::write(record_path.with_extension("inputs"), inputs).expect("write kernel assembly inputs");
    let linker = env::current_exe().expect("locate kernel build script");
    println!(
        "cargo:rustc-env=BREENIX_KERNEL_BAS_LINKER={}",
        linker.display()
    );
    println!(
        "cargo:rustc-link-arg-bin={bin}={LINK_RECORD_ARG}{}",
        record_path.display()
    );
}

fn assemble_aarch64(kernel_dir: &Path, out_dir: &Path, use_bas: bool) {
    let repo_root = kernel_dir.parent().expect("kernel directory parent");
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("scripts/kernel-link.sh").display()
    );
    let record_path = out_dir.join(ASSEMBLY_RECORD);
    remove_record(&record_path);
    if !use_bas {
        remove_record(&record_path.with_extension("inputs"));
        return;
    }

    let (bas, mut record) = build_host_bas(&repo_root.join("toolchain"), out_dir);
    let source_dir = kernel_dir.join(AARCH64_ASSEMBLY_DIR);
    let guard = fs::read(source_dir.join(AARCH64_GUARD)).expect("read resume-PC guard");
    writeln!(
        record,
        "{AARCH64_ASSEMBLY_DIR}/{AARCH64_GUARD}={}",
        digest_bytes(&guard)
    )
    .unwrap();

    for (source, unit, object) in AARCH64_ASSEMBLY {
        let input = source_dir.join(source);
        let unit_path = out_dir.join(unit);
        let object_path = out_dir.join(object);
        let source_bytes =
            fs::read(&input).unwrap_or_else(|error| panic!("read {}: {error}", input.display()));
        let unit_bytes = [guard.as_slice(), &source_bytes].concat();
        fs::write(&unit_path, &unit_bytes).expect("write ARM64 assembly unit");

        let status = Command::new(&bas)
            .args(["--target", "aarch64", "-o"])
            .arg(&object_path)
            .arg(&unit_path)
            .status()
            .expect("run host bas for ARM64");
        assert!(status.success(), "assemble {}", unit_path.display());
        writeln!(
            record,
            "{AARCH64_ASSEMBLY_DIR}/{source}={}\n{unit}={}\n{object}={}",
            digest_bytes(&source_bytes),
            digest_bytes(&unit_bytes),
            digest(&object_path)
        )
        .unwrap();
        println!(
            "cargo:rustc-link-arg-bin=kernel-aarch64={}",
            object_path.display()
        );
    }

    println!("cargo:rustc-cfg=breenix_bas_kernel_asm");
    record_kernel_link(&record_path, "kernel-aarch64", record);
}

fn assemble_x86(kernel_dir: &Path, out_dir: &Path, use_bas: bool) {
    let repo_root = kernel_dir.parent().expect("kernel directory parent");
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("scripts/kernel-link.sh").display()
    );
    let record_path = out_dir.join(ASSEMBLY_RECORD);
    remove_record(&record_path);
    let mut bas = use_bas.then(|| build_host_bas(&repo_root.join("toolchain"), out_dir));

    for (source, object) in X86_ASSEMBLY {
        let output = out_dir.join(object);
        let input = kernel_dir.join(source);
        let mut assembler = match &bas {
            Some((path, _)) => {
                let mut command = Command::new(path);
                command.args(["--target", "x86_64"]);
                command
            }
            None => {
                let mut command = Command::new("nasm");
                command.args(["-f", "elf64"]);
                command
            }
        };
        let status = assembler
            .arg("-o")
            .arg(&output)
            .arg(&input)
            .status()
            .unwrap_or_else(|error| panic!("run {assembler:?}: {error}"));
        assert!(status.success(), "assemble {}", input.display());
        if let Some((_, record)) = &mut bas {
            writeln!(
                record,
                "{source}={}\n{object}={}",
                digest(&input),
                digest(&output)
            )
            .unwrap();
        }
    }

    for object in [
        "syscall_entry.o",
        "ap_trampoline.o",
        "timer_entry.o",
        "breakpoint_entry.o",
    ] {
        println!("cargo:rustc-link-arg={}", out_dir.join(object).display());
    }
    if let Some((_, record)) = bas {
        record_kernel_link(&record_path, "kernel", record);
    }
}

fn main() {
    let mut args = env::args_os().skip(1);
    if args.next().as_deref() == Some(OsStr::new("--link")) {
        link_kernel(args.collect());
        return;
    }
    println!("cargo:rerun-if-env-changed={ASSEMBLER}");
    println!("cargo:rustc-check-cfg=cfg(breenix_bas_kernel_asm)");
    let selection = env::var_os(ASSEMBLER);
    let use_bas = match selection.as_deref().and_then(OsStr::to_str) {
        None if selection.is_none() => false,
        Some("nasm") => false,
        Some("bas") => true,
        _ => panic!("{ASSEMBLER} must be nasm or bas, got {selection:?}"),
    };

    // Emit a unique build ID based on current timestamp (seconds + subsecond nanos).
    // Baked into the kernel boot banner so stale builds are immediately detectable.
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let build_id = format!(
        "{:010x}{:04x}",
        ts.as_secs(),
        (ts.subsec_nanos() >> 16) & 0xFFFF
    );
    println!("cargo:rustc-env=BREENIX_BUILD_ID={}", build_id);
    // Rerun whenever ANY kernel source file changes, so BUILD_ID always
    // reflects the actual build and never lies about staleness.
    //
    // NOTE: emitting any `cargo:rerun-if-changed` disables cargo's default
    // "rerun if anything in the package changed" behavior, so without this
    // directive BUILD_ID only re-stamped on the few files explicitly listed
    // below (e.g. xhci.rs) -- a stale BUILD_ID silently misled a validation
    // run once. Cargo recursively watches the whole tree when given a
    // directory path, so this restores honest "any source change" coverage.
    println!("cargo:rerun-if-changed=src");

    // Get absolute paths from Cargo environment
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let kernel_dir = PathBuf::from(&manifest_dir);
    let target = env::var("TARGET").unwrap_or_default();

    // The x86 build consumes committed 7.1 outputs (G5); no assembler runs here.
    if target.starts_with("x86_64") {
        for path in ["assets/kepler", "verify_kepler_firmware.py", "../docs/gui/provenance/kepler-toolchain.sha256.json"] {
            println!("cargo:rerun-if-changed={path}");
        }
        let status = Command::new("python3")
            .arg(kernel_dir.join("verify_kepler_firmware.py"))
            .arg(kernel_dir.parent().expect("kernel directory parent"))
            .status()
            .expect("Kepler asset verification requires Python 3");
        assert!(status.success(), "Kepler firmware verification failed; run cargo run -p xtask -- kepler-assets --check and commit kernel/assets/kepler/");
    }

    // Supply the fallback fixture for noninteractive testing boots without an
    // attached home disk. Interactive profiles use their writable home disk.
    if env::var_os("CARGO_FEATURE_TESTING").is_some()
        && env::var_os("CARGO_FEATURE_INTERACTIVE").is_none()
    {
        let fixture = PathBuf::from(&out_dir).join("readonly-fixture");
        fs::create_dir_all(&fixture).expect("create read-only fixture directory");
        fs::write(fixture.join(".breenix-readonly-fixture"), b"Read-only ext2 acceptance fixture\n")
            .expect("write fixture marker");
        fs::write(fixture.join("hello.txt"), b"Read-only ext2 fixture contents\n")
            .expect("write fixture contents");
        let mke2fs = if env::consts::OS == "macos" {
            let prefix = Command::new("brew").args(["--prefix", "e2fsprogs"])
                .output().expect("read-only fixture needs e2fsprogs");
            assert!(prefix.status.success(), "read-only fixture needs e2fsprogs");
            PathBuf::from(String::from_utf8(prefix.stdout).unwrap().trim()).join("sbin/mke2fs")
        } else {
            PathBuf::from("mke2fs")
        };
        let image = PathBuf::from(&out_dir).join("readonly-ext2.img");
        fs::File::create(&image).expect("create fixture image").set_len(1024 * 1024)
            .expect("size fixture image");
        let status = Command::new(mke2fs)
            .args(["-q", "-t", "ext2", "-F", "-b", "1024", "-I", "256", "-N", "32", "-m", "0", "-O", "none", "-d"])
            .arg(fixture).arg(image).status().expect("build read-only ext2 fixture");
        assert!(status.success(), "build read-only ext2 fixture");
    }

    // The boot-path stage markers, so the kernel's output paths can keep the
    // lines a stage depends on whole (`stage_lines`).
    let repo_root = kernel_dir.parent().expect("kernel dir should have parent");
    for source in stage_markers::sources(repo_root) {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    let markers = stage_markers::boot_path_markers(repo_root)
        .unwrap_or_else(|error| panic!("boot-path stage markers: {error}"));
    fs::write(
        PathBuf::from(&out_dir).join("stage_markers.rs"),
        format!("&{markers:?}"),
    )
    .expect("write stage_markers.rs");

    if target.starts_with("aarch64") {
        let ribbon=stage_markers::arm64_ribbon(repo_root).expect("ARM64 ribbon stages");
        assert!(!ribbon.is_empty() && ribbon.len() <= 416, "ARM64 boot ribbon exceeds its 1248-pixel segment capacity");
        fs::write(PathBuf::from(&out_dir).join("boot_ribbon.rs"),format!("&{ribbon:?}")).expect("write ribbon stages");
    } else if target.starts_with("x86_64") {
        let ribbon = stage_markers::metal_ribbon(repo_root).expect("metal ribbon stages");
        assert!(!ribbon.is_empty() && ribbon.len() <= 416, "metal boot ribbon exceeds segment capacity");
        fs::write(PathBuf::from(&out_dir).join("boot_ribbon.rs"), format!("&{ribbon:?}")).expect("write metal ribbon stages");
    }

    if target.contains("x86_64") {
        assemble_x86(&kernel_dir, Path::new(&out_dir), use_bas);
    }

    // Use our custom linker script for x86_64
    // Temporarily disabled to test with bootloader's default
    // println!("cargo:rustc-link-arg=-Tkernel/linker.ld");

    // AArch64 kernel linker script (moved from aarch64-breenix.json so the
    // target JSON can be shared between kernel and userspace std builds)
    if target.contains("aarch64") {
        assemble_aarch64(&kernel_dir, Path::new(&out_dir), use_bas);
        println!("cargo:rustc-link-arg=-Tkernel/src/arch_impl/aarch64/linker.ld");
        println!("cargo:rustc-link-arg=--fix-cortex-a53-843419");
        println!("cargo:rustc-link-arg=--orphan-handling=error");
    }

    // Rerun if the assembly files change
    println!("cargo:rerun-if-changed=src/syscall/entry.asm");
    println!("cargo:rerun-if-changed=src/interrupts/timer_entry.asm");
    println!("cargo:rerun-if-changed=src/interrupts/breakpoint_entry.asm");
    println!("cargo:rerun-if-changed=src/arch_impl/x86_64/ap_trampoline.asm");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=src/arch_impl/aarch64/linker.ld");

    // Userspace is built by the launchers and xtask before packing their disks.
    // Userspace Cargo builds use the shared cache and must stay outside this
    // script. The isolated, offline host bas workspace above is independent.
}
