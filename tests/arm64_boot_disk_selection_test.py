#!/usr/bin/env python3
"""Run ARM64 boot disk selection with auxiliary fixture/scratch disks visible."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def production_function(path, name):
    source = (ROOT / path).read_text()
    start = source.index(f"fn {name}(")
    opening = source.index("{", start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    # Compile the ARM64 branches on either host architecture. Only cfg
    # predicates change; device discovery and selection bodies are verbatim.
    return (source[start:end]
            .replace('target_arch = "x86_64"', 'feature = "host_x86"')
            .replace('target_arch = "aarch64"', 'feature = "host_arm64"'))


harness = r'''
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::{block::BlockDevice, fs::errno::Errno,
    fs::vfs::{probe_mount, BootMount}, syscall::errno::*};

static COUNT: AtomicUsize = AtomicUsize::new(0);
static SELECTED: AtomicUsize = AtomicUsize::new(usize::MAX);

mod block {
    pub trait BlockDevice { fn index(&self) -> usize; }
    pub mod virtio {
        pub struct VirtioBlockWrapper(usize);
        impl VirtioBlockWrapper {
            pub fn new(index: usize) -> Option<Self> {
                (index < crate::COUNT.load(core::sync::atomic::Ordering::Relaxed))
                    .then_some(Self(index))
            }
        }
        impl super::BlockDevice for VirtioBlockWrapper {
            fn index(&self) -> usize { self.0 }
        }
    }
}
mod drivers {
    pub mod ahci {
        pub fn sata_device_count() -> usize { 0 }
        pub fn get_block_device_by_index(_: usize)
            -> Option<crate::block::virtio::VirtioBlockWrapper> { None }
    }
}
mod syscall { pub mod errno { pub const ENODEV: i32 = 19; } }
mod fs {
    pub const FORMATS: &[()] = &[];
    pub mod errno { #[derive(Debug)] pub struct Errno(pub i32); }
    pub mod vfs {
        pub enum BootMount { Root, Home }
        pub fn probe_mount(_: &[()], device: alloc::boxed::Box<dyn crate::block::BlockDevice>,
            _: BootMount) -> Result<(), super::errno::Errno> {
            crate::SELECTED.store(device.index(), core::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
    }
}

@FUNCTIONS@

#[test]
fn auxiliary_disks_preserve_root_home_and_fault_positions() {
    let mut actual = Vec::new();
    for count in [2, 3, 4] {
        COUNT.store(count, Ordering::Relaxed);
        SELECTED.store(usize::MAX, Ordering::Relaxed);
        mount_root().map_err(|e| e.0).unwrap();
        let root = SELECTED.load(Ordering::Relaxed);
        SELECTED.store(usize::MAX, Ordering::Relaxed);
        mount_home().map_err(|e| e.0).unwrap();
        let home = SELECTED.load(Ordering::Relaxed);
        let fault = open_fault_device().expect("fault device").index();
        println!("visible_devices={count} root={root} home={home} fault={fault}");
        actual.push((count, root, home, fault));
    }
    assert_eq!(actual, [(2, 0, 1, 0), (3, 0, 1, 0), (4, 0, 1, 0)],
        "auxiliary disks must not replace the boot or fault-injection devices");
}
'''

functions = "\n\n".join([
    production_function("kernel/src/fs/vfs/boot.rs", "mount_root"),
    production_function("kernel/src/fs/vfs/boot.rs", "mount_home"),
    production_function("kernel/src/fs/vfs/fault.rs", "open_fault_device"),
])
with tempfile.TemporaryDirectory(prefix="arm64-boot-disks-") as temporary:
    path = Path(temporary)
    source, binary = path / "selection.rs", path / "selection"
    source.write_text(harness.replace("@FUNCTIONS@", functions))
    priority = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
    subprocess.run(priority + [os.environ.get("RUSTC", "rustc"), "--edition=2021",
                   "-Dwarnings", "--cfg", 'feature="host_arm64"', "--test",
                   str(source), "-o", str(binary)], check=True)
    subprocess.run([str(binary), "--nocapture"], check=True)
