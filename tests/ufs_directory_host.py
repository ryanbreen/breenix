#!/usr/bin/env python3
"""Execute the production User cwd dispatch for an existing and absent node."""
from pathlib import Path
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = Path(os.environ.get("UFS_NAMEI_SOURCE", ROOT / "kernel/src/fs/namei.rs")).read_text()
start = source.index("        Target::User", source.index("pub fn working_dir<"))
end = source.index("        Target::Absent", start)
production = source[start:end]
harness = r'''
use std::sync::Arc;
const ENOENT: u32 = 2;
mod syscall { pub mod errno { pub const EOPNOTSUPP: u32 = 95; } }
mod fs {
    pub mod permissions {
        pub struct Credentials;
        impl Credentials { pub fn current(_: bool) -> Result<Self,u64> { Ok(Self) } }
    }
    pub mod ufs { pub mod file {
        pub struct UfsFile;
        impl UfsFile {
            pub fn working_directory(_:u64, _:u64, _:Option<u64>, _: &super::super::permissions::Credentials)
                -> Result<std::sync::Arc<()>,u64> { Ok(std::sync::Arc::new(())) }
        }
    } }
    pub mod namei {
        use crate::*;
        pub enum Target { User { mount:u64, node:u64, parent:Option<u64> }, Other }
        #[derive(Debug)]
        pub enum WorkingDir { User { mount:u64, node:u64, handle:Arc<()> } }
        pub fn working_dir(target: Target) -> Result<WorkingDir,u64> {
            match target {
                PRODUCTION
                Target::Other => panic!("only User dispatch is under test"),
            }
        }
    }
}
#[test]
fn existing_user_directory_can_become_cwd_and_absent_node_stays_enoent() {
    use fs::namei::*;
    let _ = Target::Other;
    if let Target::User { mount, parent, .. } = (Target::User { mount:7, node:1, parent:None }) {
        assert_eq!((mount,parent), (7,None));
    }
    let _retained = WorkingDir::User { mount:7, node:1, handle:Arc::new(()) };
    // Keep the old-only and fixed-only stubs used in either source state.
    let _ = syscall::errno::EOPNOTSUPP;
    let _current = fs::permissions::Credentials::current;
    let _open = fs::ufs::file::UfsFile::working_directory;
    let WorkingDir::User { mount, node, handle } = working_dir(Target::User { mount:7, node:1, parent:None }).unwrap();
    assert_eq!((mount,node), (7,1));
    assert_eq!(Arc::strong_count(&handle), 1);
    assert_eq!(working_dir(Target::User { mount:7, node:0, parent:Some(1) }).unwrap_err(), 2);
}
'''.replace("PRODUCTION", production)
with tempfile.TemporaryDirectory(prefix="breenix-ufs-cwd-") as temporary:
    path = Path(temporary)
    test = path / "check.rs"
    test.write_text(harness)
    subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "-Dwarnings",
                    "--test", str(test), "-o", str(path / "check")], check=True)
    subprocess.run([str(path / "check")], check=True)
