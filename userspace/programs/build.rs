use std::{env, path::PathBuf};

fn main() {
    // libc is built separately by build.sh and linked as a native archive.
    // Cargo otherwise considers these binaries fresh when only libc changes.
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("target architecture");
    let archive = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"))
        .join(format!(
            "../../libs/libbreenix-libc/target/{arch}-breenix/release/libc.a"
        ));
    println!("cargo:rerun-if-changed={}", archive.display());
}
