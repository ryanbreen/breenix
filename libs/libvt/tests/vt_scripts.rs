#[path = "../../../userspace/programs/src/vt_script_test/scripts.rs"]
mod scripts;
use libvt::{Color, TermEmu};

#[test]
fn scripts_produce_the_guest_screen_literals() {
    for (name, lines, red) in [
        ("vt-screen", ["Hello red!", "moved", "", ""], Some((6, 9))),
        ("scroll-region", ["top", "two", "three", "bottom"], None),
        ("alt-screen", ["main restored", "", "", ""], None),
        ("utf8", ["Aé界�B", "", "", ""], None),
    ] {
        let mut vt = TermEmu::new(40, 10);
        for byte in scripts::script(name).expect("named script") { vt.feed(&[*byte]); }
        for (y, expected) in lines.iter().enumerate() {
            let line: String = (0..40).map(|x| vt.cell(x, y).ch).collect();
            assert_eq!(line.trim_end(), *expected, "{name} row {y}");
            for x in 0..40 {
                let cell = vt.cell(x, y);
                let expected_fg = if y == 0 && red.is_some_and(|(start, end)| (start..end).contains(&x)) { Color::rgb(205,49,49) } else { Color::rgb(204,204,204) };
                assert_eq!(cell.fg, expected_fg, "{name} at {x},{y}");
                assert_eq!((cell.bg, cell.bold, cell.underline, cell.inverse), (Color::rgb(30,30,40), false, false, false));
            }
        }
        assert!(!vt.alternate_screen(), "{name}");
    }
}
