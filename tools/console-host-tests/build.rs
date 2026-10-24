fn main() {
    let fixture = "../../userspace/programs/src/suite_breengel/stage_boot_markers.in";
    println!("cargo:rerun-if-changed={fixture}");
    std::fs::copy(fixture, std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("boot_ribbon.rs"))
        .expect("copy independent ARM64 stage expectations");
    // Independent endpoint fixture for the included runtime scanner tests.
    std::fs::write(std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("boot_ribbon_last.rs"),
        r#"&[("tests", "FBINFO_TEST: all tests PASSED"), ("shell", "bsh />"), ("probe", "PROBE framebuffer OK")]"#)
        .expect("write mode endpoint expectations");
}
