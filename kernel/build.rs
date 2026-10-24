use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "stage_markers.rs"]
mod stage_markers;

fn main() {
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
        let last = stage_markers::arm64_ribbon_last(repo_root).expect("ARM64 mode endpoints");
        fs::write(PathBuf::from(&out_dir).join("boot_ribbon_last.rs"), format!("&{last:?}")).expect("write mode endpoints");
        let ribbon=stage_markers::arm64_ribbon(repo_root).expect("ARM64 ribbon stages");
        assert!(!ribbon.is_empty() && ribbon.len() <= 416, "ARM64 boot ribbon exceeds its 1248-pixel segment capacity");
        fs::write(PathBuf::from(&out_dir).join("boot_ribbon.rs"),format!("&{ribbon:?}")).expect("write ribbon stages");
    }

    // Only build x86_64 assembly for x86_64 targets
    if target.contains("x86_64") {
        // Assemble syscall entry code
        let status = Command::new("nasm")
            .args(&[
                "-f",
                "elf64",
                "-o",
                &format!("{}/syscall_entry.o", out_dir),
                kernel_dir.join("src/syscall/entry.asm").to_str().unwrap(),
            ])
            .status()
            .expect("Failed to run nasm");

        if !status.success() {
            panic!("Failed to assemble syscall entry");
        }

        // Assemble timer interrupt entry code
        let status = Command::new("nasm")
            .args(&[
                "-f",
                "elf64",
                "-o",
                &format!("{}/timer_entry.o", out_dir),
                kernel_dir
                    .join("src/interrupts/timer_entry.asm")
                    .to_str()
                    .unwrap(),
            ])
            .status()
            .expect("Failed to run nasm");

        if !status.success() {
            panic!("Failed to assemble timer entry");
        }

        // Assemble breakpoint exception entry code
        let status = Command::new("nasm")
            .args(&[
                "-f",
                "elf64",
                "-o",
                &format!("{}/breakpoint_entry.o", out_dir),
                kernel_dir
                    .join("src/interrupts/breakpoint_entry.asm")
                    .to_str()
                    .unwrap(),
            ])
            .status()
            .expect("Failed to run nasm");

        if !status.success() {
            panic!("Failed to assemble breakpoint entry");
        }

        // Assemble the application-processor startup trampoline
        let status = Command::new("nasm")
            .args(&[
                "-f",
                "elf64",
                "-o",
                &format!("{}/ap_trampoline.o", out_dir),
                kernel_dir
                    .join("src/arch_impl/x86_64/ap_trampoline.asm")
                    .to_str()
                    .unwrap(),
            ])
            .status()
            .expect("Failed to run nasm");

        if !status.success() {
            panic!("Failed to assemble AP trampoline");
        }

        // Tell cargo to link the assembled object files
        println!("cargo:rustc-link-arg={}/syscall_entry.o", out_dir);
        println!("cargo:rustc-link-arg={}/ap_trampoline.o", out_dir);
        println!("cargo:rustc-link-arg={}/timer_entry.o", out_dir);
        println!("cargo:rustc-link-arg={}/breakpoint_entry.o", out_dir);
    }

    // Use our custom linker script for x86_64
    // Temporarily disabled to test with bootloader's default
    // println!("cargo:rustc-link-arg=-Tkernel/linker.ld");

    // AArch64 kernel linker script (moved from aarch64-breenix.json so the
    // target JSON can be shared between kernel and userspace std builds)
    if target.contains("aarch64") {
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
    // Never invoke Cargo (including through build.sh) from a build script: the
    // parent may hold the package-cache mutation lock for the entire build.
}
