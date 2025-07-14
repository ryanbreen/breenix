//! Breenix xtask - Build orchestration CLI for Breenix OS
//! 
//! This tool provides commands for building the kernel and running tests
//! in a way that's compatible with cross-compilation constraints.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

mod build;
mod qemu;
mod test;

pub use build::*;
pub use qemu::*;
pub use test::*;

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Breenix build and test orchestration")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the kernel with specified features
    Build {
        /// Features to enable (comma-separated)
        #[arg(long, default_value = "testing")]
        features: String,
        /// Build in release mode
        #[arg(long)]
        release: bool,
    },
    /// Run QEMU with a kernel image
    RunQemu {
        /// Path to kernel image
        kernel_img: PathBuf,
        /// Timeout in seconds
        #[arg(long, default_value = "30")]
        timeout: u64,
    },
    /// Build kernel and run QEMU in one step  
    BuildAndRun {
        /// Features to enable (comma-separated)
        #[arg(long, default_value = "testing")]
        features: String,
        /// Build in release mode
        #[arg(long)]
        release: bool,
        /// Timeout in seconds
        #[arg(long, default_value = "30")]
        timeout: u64,
    },
    /// Run all tests in a single QEMU boot (fast workflow B)
    TestAll {
        /// Timeout in seconds
        #[arg(long, default_value = "60")]
        timeout: u64,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Build { features, release } => {
            let features: Vec<&str> = features.split(',').collect();
            let kernel_img = build_kernel(&features, release)?;
            println!("✅ Kernel built successfully: {}", kernel_img.display());
            Ok(())
        }
        Commands::RunQemu { kernel_img, timeout } => {
            let outcome = run_qemu(&kernel_img, Duration::from_secs(timeout))?;
            println!("🚀 QEMU execution completed");
            println!("Exit code: {:?}", outcome.exit_code);
            println!("Duration: {:?}", outcome.duration);
            println!("Output length: {} bytes", outcome.serial_output.len());
            Ok(())
        }
        Commands::BuildAndRun { features, release, timeout } => {
            let features: Vec<&str> = features.split(',').collect();
            let kernel_img = build_kernel(&features, release)?;
            let outcome = run_qemu(&kernel_img, Duration::from_secs(timeout))?;
            println!("🚀 QEMU execution completed");
            println!("Exit code: {:?}", outcome.exit_code);
            println!("Duration: {:?}", outcome.duration);
            println!("Output length: {} bytes", outcome.serial_output.len());
            
            // Show actual kernel output to verify what we're capturing
            if !outcome.serial_output.is_empty() {
                println!("\n📄 ACTUAL KERNEL OUTPUT:");
                println!("========================");
                println!("{}", outcome.serial_output);
                println!("========================\n");
            } else {
                println!("⚠️  No kernel output captured!");
            }
            Ok(())
        }
        Commands::TestAll { timeout } => {
            test_all(Duration::from_secs(timeout))
        }
    }
}