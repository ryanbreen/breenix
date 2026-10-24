fn main() {
    let fixture = "../../userspace/programs/src/suite_breengel/stage_boot_markers.in";
    println!("cargo:rerun-if-changed={fixture}");
    std::fs::copy(fixture, std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("boot_ribbon.rs"))
        .expect("copy independent ARM64 stage expectations");
}
