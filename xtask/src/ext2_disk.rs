//! Ext2 Disk Image Builder
//!
//! Creates an ext2 filesystem image for testing the Breenix ext2 driver.
//! Uses Docker with Alpine Linux to access ext2 tools (mke2fs, mount, etc.)
//! since macOS doesn't have native ext2 support.
//!
//! ## Disk Layout
//!
//! The image is a standard ext2 filesystem containing:
//! - `/hello.txt` - Test file with "Hello from ext2!\n"
//! - `/test/nested.txt` - Nested file with "Nested file content\n"
//!
//! ## Usage
//!
//! ```bash
//! cargo run -p xtask -- create-ext2-disk
//! ```
//!
//! Creates `target/ext2.img` (4MB ext2 filesystem).

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};

const EXT2_IMAGE_SIZE_MB: u32 = 4;
const EXT2_IMAGE_NAME: &str = "ext2.img";

/// Creates an ext2 disk image with test files using Docker.
///
/// The image will be created at `target/ext2.img`.
pub fn create_ext2_disk() -> Result<()> {
    println!("Creating ext2 disk image...");

    let target_dir = Path::new("target");
    let output_path = target_dir.join(EXT2_IMAGE_NAME);

    // Ensure target directory exists
    if !target_dir.exists() {
        fs::create_dir_all(target_dir)?;
    }

    // Check if Docker is available and daemon is running
    let docker_check = Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output();

    match docker_check {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("  Docker is available (server version: {})", version.trim());
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "Docker daemon is not running.\n\
                 Error: {}\n\
                 Start Docker Desktop and try again.",
                stderr.trim()
            );
        }
        Err(e) => {
            bail!(
                "Docker is required to create ext2 images on macOS.\n\
                 Error: {}\n\
                 Install Docker Desktop: https://docs.docker.com/desktop/mac/install/",
                e
            );
        }
    }

    // Get absolute path for Docker volume mount
    let abs_target_dir = fs::canonicalize(target_dir)
        .unwrap_or_else(|_| target_dir.to_path_buf());

    // Docker command to create ext2 image
    // Uses Alpine Linux which has small footprint and includes e2fsprogs
    let docker_script = format!(
        r#"
set -e

# Create the empty disk image
dd if=/dev/zero of=/work/{} bs=1M count={} status=none

# Create ext2 filesystem
mke2fs -t ext2 -F /work/{} >/dev/null 2>&1

# Mount and populate
mkdir -p /mnt/ext2
mount /work/{} /mnt/ext2

# Create test files
echo "Hello from ext2!" > /mnt/ext2/hello.txt
mkdir -p /mnt/ext2/test
echo "Nested file content" > /mnt/ext2/test/nested.txt

# Create some additional test content
mkdir -p /mnt/ext2/deep/path/to/file
echo "Deep nested content" > /mnt/ext2/deep/path/to/file/data.txt

# Show what was created
echo "Files created:"
find /mnt/ext2 -type f -exec ls -la {{}} \;

# Unmount
umount /mnt/ext2

echo "ext2 image created successfully"
"#,
        EXT2_IMAGE_NAME,
        EXT2_IMAGE_SIZE_MB,
        EXT2_IMAGE_NAME,
        EXT2_IMAGE_NAME
    );

    println!("  Running Docker to create ext2 filesystem...");

    // Run Docker with privileged mode (needed for mount)
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--privileged",
            "-v",
            &format!("{}:/work", abs_target_dir.display()),
            "alpine:latest",
            "sh",
            "-c",
            &format!("apk add --no-cache e2fsprogs >/dev/null 2>&1 && {}", docker_script),
        ])
        .status()?;

    if !status.success() {
        bail!("Failed to create ext2 image via Docker");
    }

    // Verify the image was created
    if !output_path.exists() {
        bail!("ext2 image was not created at {}", output_path.display());
    }

    let image_size = fs::metadata(&output_path)?.len();
    println!("\next2 disk created: {}", output_path.display());
    println!("  Size: {} bytes ({:.2} MB)", image_size, image_size as f64 / (1024.0 * 1024.0));
    println!("  Filesystem: ext2");
    println!("  Contents:");
    println!("    /hello.txt - \"Hello from ext2!\"");
    println!("    /test/nested.txt - \"Nested file content\"");
    println!("    /deep/path/to/file/data.txt - \"Deep nested content\"");

    Ok(())
}
