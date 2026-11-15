use std::{env, fs, path::PathBuf};

fn main() {
    let source = "../../kernel/src/syscall/fs.rs";
    println!("cargo:rerun-if-changed={source}");
    let text = fs::read_to_string(source).unwrap();
    let start = text
        .find("#[repr(C)]\n#[derive(Clone, Copy, Default)]\nstruct Statfs")
        .unwrap();
    let end = text[start..].find("pub fn sys_statfs(").unwrap() + start;
    let user = text.find("fn user_statfs(").unwrap();
    // Compile the production dispatch and UFS decoder, with namespace and
    // daemon transport supplied by the host fixture.
    let code = format!("{}\n{}", &text[start..end], &text[user..])
        .replace("crate::fs::vfs::mount::", "crate::statfs_support::mount::");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("statfs_dispatch.rs"),
        code,
    )
    .unwrap();
}
