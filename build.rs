// build.rs

use bootloader::DiskImageBuilder;
use std::{env, path::PathBuf};

fn main() {
    // set by cargo for the kernel artifact dependency
    let kernel_path = env::var("CARGO_BIN_FILE_KERNEL").unwrap();
    let disk_builder = DiskImageBuilder::new(PathBuf::from(kernel_path));

    // specify output paths
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let uefi_path = out_dir.join("breenix-uefi.img");
    let bios_path = out_dir.join("breenix-bios.img");

    // Only create the UEFI image by default. BIOS image can be enabled via env var.
    println!("cargo:warning=Creating UEFI disk image at {}", uefi_path.display());
    disk_builder
        .create_uefi_image(&uefi_path)
        .expect("failed to create UEFI disk image");

    let build_bios = env::var("BREENIX_BUILD_BIOS").is_ok();
    if build_bios {
        println!(
            "cargo:warning=BREENIX_BUILD_BIOS set; creating BIOS disk image at {}",
            bios_path.display()
        );
        // New bootloader API removed BIOS builder; use UEFI image as placeholder to keep API surface stable.
        // If BIOS support is needed, switch to a branch that still exposes create_bios_image or vendor our own.
        println!("cargo:warning=bootloader no longer provides create_bios_image; duplicating UEFI image for BIOS placeholder");
        disk_builder
            .create_uefi_image(&bios_path)
            .expect("failed to create BIOS placeholder image");
    } else {
        println!("cargo:warning=Skipping BIOS image creation (BREENIX_BUILD_BIOS not set)");
    }

    // pass the disk image paths via environment variables
    println!("cargo:rustc-env=UEFI_IMAGE={}", uefi_path.display());
    println!("cargo:rustc-env=BIOS_IMAGE={}", bios_path.display());
}