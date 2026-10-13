//! Compile the production number declarations without the kernel dispatcher.
fn main() {
    let source = "../../kernel/src/syscall/mod.rs";
    println!("cargo:rerun-if-changed={source}");
    let text = std::fs::read_to_string(source).unwrap();
    let declarations = text
        .split_once("// Linux IPC number constants.\n")
        .unwrap()
        .1;
    let declarations = declarations.split_once("// End Linux IPC number constants.").unwrap().0;
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(output.join("ipc_numbers.rs"), declarations).unwrap();
}
