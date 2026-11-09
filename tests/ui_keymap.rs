//! Keyboard scripts exercise the literal map and production WM translation.
extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libuitest/src/input.rs"]
pub mod input;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;
use input::{Delivery, Keyboard, KeyboardBackend, Modifier, Target};
use std::{fs, process::Command};
fn function<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source.find(name).expect(name);
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
    &source[start..=brace + end]
}
#[test]
fn literal_map_round_trips_every_printable_ascii_through_bwm() {
    let mut program = format!(
        "use libbreenix::input; {}\nfn main() {{\n",
        function(
            include_str!("../userspace/programs/src/bwm/input_keys.rs"),
            "fn key_text("
        )
    );
    // Compile the WM's own function against the production input constants.
    program = program.replace(
        "use libbreenix::input;",
        "mod input { pub use input_abi::*; } mod libbreenix {pub use crate::input;}",
    );
    program = program.replace("mod input {", "pub mod input {");
    for byte in 32..=126u8 {
        let stroke = input::key_stroke(byte as char).expect("printable literal entry");
        program.push_str(&format!(
            "assert_eq!(key_text({},{}),Some({byte}),\"ASCII {byte}\");\n",
            stroke.code,
            if stroke.shift { 1 } else { 0 }
        ));
    }
    program.push_str("}\n");
    let dir = std::env::temp_dir().join(format!("ui-keymap-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    // Compile the ABI as its own crate, preserving its no_std crate attribute.
    let abi = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("libs/input-abi/src/lib.rs");
    program = format!("extern crate input_abi;\n{program}");
    fs::write(dir.join("map.rs"), program).unwrap();
    let abi_build = Command::new("rustc")
        .args(["--edition=2021", "-Dwarnings", "--crate-type=lib", "--crate-name=input_abi"])
        .arg(&abi).arg("-o").arg(dir.join("libinput_abi.rlib")).output().unwrap();
    assert!(abi_build.status.success(), "{}", String::from_utf8_lossy(&abi_build.stderr));
    let build = Command::new("rustc")
        .args(["--edition=2021", "-Dwarnings"])
        .arg("--extern").arg(format!("input_abi={}",dir.join("libinput_abi.rlib").display()))
        .arg(dir.join("map.rs"))
        .arg("-o")
        .arg(dir.join("map"))
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(dir.join("map")).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(input::key_stroke('\n').unwrap().code, 28);
    assert_eq!(input::key_stroke('\t').unwrap().code, 15);
    assert!(input::key_stroke('λ').is_none());
}
use std::path::PathBuf;
#[derive(Default)]
struct Fake {
    log: Vec<String>,
    held: Vec<u16>,
    fail: Option<u16>,
    focusable: bool,
}
impl KeyboardBackend for Fake {
    fn key(&mut self, code: u16, pressed: bool, window: u64) -> Result<Delivery, String> {
        self.log.push(format!("key:{code}:{pressed}"));
        if pressed {
            self.held.push(code);
        } else {
            self.held.retain(|c| *c != code);
        }
        if pressed && self.fail == Some(code) {
            return Err("delivery failure".into());
        }
        Ok(Delivery {
            serial: self.log.len() as u64,
            window,
        })
    }
    fn text(&mut self, text: &str, window: u64) -> Result<Delivery, String> {
        self.log.push(format!("text:{text}"));
        Ok(Delivery {
            serial: self.log.len() as u64,
            window,
        })
    }
    fn focus(&mut self, target: &Target) -> Result<(), String> {
        self.log.push(format!("click:{}", target.key));
        if self.focusable {
            Ok(())
        } else {
            Err(format!("input-focus: {} cannot take focus", target.key))
        }
    }
}
#[test]
fn scripts_distinguish_keys_text_chords_and_focus_and_release_on_error() {
    let mut k = Keyboard::new(Fake::default());
    k.type_text(9, "aA!\n\tλ🙂").unwrap();
    assert_eq!(
        k.backend_mut().log,
        [
            "key:30:true",
            "key:30:false",
            "key:42:true",
            "key:30:true",
            "key:30:false",
            "key:42:false",
            "key:42:true",
            "key:2:true",
            "key:2:false",
            "key:42:false",
            "key:28:true",
            "key:28:false",
            "key:15:true",
            "key:15:false",
            "text:λ",
            "text:🙂"
        ]
    );
    k.backend_mut().log.clear();
    k.chord(9, &[Modifier::Ctrl, Modifier::Shift], 30).unwrap();
    assert_eq!(
        k.backend_mut().log,
        [
            "key:29:true",
            "key:42:true",
            "key:30:true",
            "key:30:false",
            "key:42:false",
            "key:29:false"
        ]
    );
    k.backend_mut().fail = Some(30);
    assert!(k.chord(9, &[Modifier::Ctrl], 30).is_err());
    assert!(k.backend_mut().held.is_empty());
    k.backend_mut().fail = Some(42);
    assert!(k.chord(9, &[Modifier::Shift], 30).is_err());
    assert!(k.backend_mut().held.is_empty());
    k.backend_mut().log.clear();
    assert!(k
        .focus_then_type(&Target::new(9, "status"), "x")
        .unwrap_err()
        .contains("status"));
    assert_eq!(k.backend_mut().log, ["click:status"]);
    k.backend_mut().focusable = true;
    k.backend_mut().fail = None;
    k.backend_mut().log.clear();
    k.focus_then_type(&Target::new(9, "entry"), "x").unwrap();
    assert_eq!(
        k.backend_mut().log,
        ["click:entry", "key:45:true", "key:45:false"]
    );
}
#[test]
fn delivery_watermark_orders_derivations_by_source() {
    let source = include_str!("../libs/libuitest/src/pointer_live.rs");
    assert!(source.contains("client_processed("));
    assert!(!input::client_processed((3 << 62) | (10 << 22) | 1, 12));
    assert!(input::client_processed((3 << 62) | (12 << 22) | 1, 12));
    assert!(input::client_processed(13, 12));
}
