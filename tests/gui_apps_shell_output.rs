//! Run the guest scenarios' commands through their real word parser and JS engine.
#[path = "../userspace/programs/src/bsh_words.rs"]
mod bsh_words;
use std::sync::Mutex;
static OUTPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
fn capture(s: &str) { OUTPUT.lock().unwrap().extend_from_slice(s.as_bytes()); }

#[test]
fn exit_status_command_produces_one() {
    let source = include_str!("../userspace/programs/src/guiapps_suite/bterm.rs");
    let exit = source.split("pub fn exit_status()").nth(1).unwrap()
        .split("t.type_line(\"").nth(1).unwrap().split("\")?").next().unwrap();
    let words = bsh_words::words(exit).unwrap();
    let output = std::process::Command::new(&words[0]).args(&words[1..]).output()
        .expect("the scenario must name an executable, not a POSIX command list passed as argv");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1\n");

}

#[test]
fn scrollback_command_preserves_page_alignment() {
    let source = include_str!("../userspace/programs/src/guiapps_suite/bterm.rs");
    let scroll = source.split("pub fn scrollback()").nth(1).unwrap()
        .split("t.type_line(&format!(\"").nth(1).unwrap().split("\"))?").next().unwrap();
    for rows in [20, 25, 28, 30, 32] {
        let padding = (rows - 201 % rows) % rows;
        let command = scroll.replace("{padding}", &padding.to_string());
        let mut context = breenish_js::Context::new();
        context.set_print_fn(capture);
        let result = context.eval(&command).unwrap();
        let mut bytes = std::mem::take(&mut *OUTPUT.lock().unwrap());
        // bsh's REPL prints a non-undefined evaluation result after command output.
        if !result.is_undefined() {
            bytes.extend_from_slice(format!("{}\n", context.format_value(result)).as_bytes());
        }
        bytes.extend_from_slice(b"bsh /> ");
        let mut vt = libvt::TermEmu::new(70, rows);
        vt.feed(b"banner\r\nbsh /> command\r\n");
        for byte in bytes {
            if byte == b'\n' { vt.feed(b"\r"); }
            vt.feed(&[byte]);
        }
        let mut found = false;
        for _ in 0..(1000 / rows + 2) {
            let top: String = (0..70).map(|x| vt.view_cell(x, 0).ch).collect();
            if top.trim_end() == "1" { found = true; break; }
            vt.scroll_view(rows as isize);
        }
        assert!(found, "page up never places line 1 at the top for {rows} rows");
    }
    assert_eq!(bsh_words::js_string("$?"), "\"$?\"");
}
