//! Exercise the kernel's publication flags, including the uinput call site.
use std::{fs, process::Command};
#[test]
fn uinput_is_tagged_and_hardware_keeps_only_derived_modifiers() {
    let source = include_str!("../kernel/src/input/stream.rs");
    let start = source
        .find("fn publication_flags(")
        .expect("kernel flag function");
    let brace = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    let end = source[brace..]
        .bytes()
        .position(|b| {
            if b == b'{' {
                depth += 1;
            }
            if b == b'}' {
                depth -= 1;
            }
            depth == 0
        })
        .unwrap();
    let function = &source[start..=brace + end];
    let abi =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("libs/input-abi/src/lib.rs");
    let fixture = format!(
        r#"extern crate input_abi;
{function}
fn main() {{
 assert_eq!(input_abi::SYNTHETIC & input_abi::modifier::MASK,0);
 for mods in 0..=15 {{
  assert_eq!(publication_flags(mods,0),mods as u32);
  assert_eq!(publication_flags(mods,input_abi::SYNTHETIC),mods as u32|input_abi::SYNTHETIC);
 }}
 assert_eq!(publication_flags(0,u32::MAX),input_abi::SYNTHETIC);
}}
"#
    );
    let dir = std::env::temp_dir().join(format!("input-synthetic-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("tag.rs"), fixture).unwrap();
    let abi_build = Command::new("rustc")
        .args(["--edition=2021", "-Dwarnings", "--crate-type=lib", "--crate-name=input_abi"])
        .arg(&abi).arg("-o").arg(dir.join("libinput_abi.rlib")).output().unwrap();
    assert!(abi_build.status.success(), "{}", String::from_utf8_lossy(&abi_build.stderr));
    let build = Command::new("rustc")
        .args(["--edition=2021", "-Dwarnings"])
        .arg("--extern").arg(format!("input_abi={}",dir.join("libinput_abi.rlib").display()))
        .arg(dir.join("tag.rs"))
        .arg("-o")
        .arg(dir.join("tag"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(dir.join("tag")).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let inject = source
        .split("fn inject(&self, id: u32,")
        .nth(1)
        .unwrap()
        .split("fn ")
        .next()
        .unwrap();
    assert!(
        inject.contains("e.flags != 0"),
        "injectors cannot forge flags"
    );
    assert!(inject.contains("e.flags = input_abi::SYNTHETIC;"));
    assert!(source.contains("let trace_flags = publication_flags(flags, e.flags);"));
    assert!(source.contains("e.flags = u32::from(flags);"));
    assert!(source.contains("pipeline::record(16, device as u64, u64::from(trace_flags), e.serial)"));
}
