use std::{env, fs, path::PathBuf};

mod mappings {
    include!("../breenfs-host-tests/build.rs");
    pub fn generate() { main(); }
}

fn main() {
    mappings::generate();
    let block = "../../kernel/src/block/mod.rs";
    println!("cargo:rerun-if-changed={block}");
    let block = fs::read_to_string(block).unwrap();
    let root_functions = ["fn named_root(", "fn report_root_record("].map(|marker| {
        let start = block.find(marker).unwrap();
        let end = start + block[start..].find("\n}").unwrap() + 2;
        &block[start..end]
    });
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("named_root.rs"),
        root_functions.join("\n").replace("crate::boot::intent::", "intent::"),
    )
    .unwrap();
    let source = "../../kernel/src/syscall/fs.rs";
    println!("cargo:rerun-if-changed={source}");
    let text = fs::read_to_string(source).unwrap();
    let start = text
        .find("#[repr(C)]\n#[derive(Clone, Copy, Default)]\nstruct Statfs")
        .unwrap();
    let end = text[start..].find("pub fn sys_statfs(").unwrap() + start;
    let user = text.find("fn user_statfs(").unwrap();
    let user_end = user + text[user..].find("\n}").unwrap() + 2;
    // Compile the production dispatch and UFS decoder, with namespace and
    // daemon transport supplied by the host fixture.
    let code = format!("{}\n{}", &text[start..end], &text[user..user_end])
        .replace("crate::fs::vfs::mount::", "crate::statfs_support::mount::");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("statfs_dispatch.rs"),
        code,
    )
    .unwrap();
}
