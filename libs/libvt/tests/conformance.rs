use libvt::{Color, TermEmu};

struct Row {
    name: &'static str,
    input: &'static [u8],
    lines: [&'static str; 2],
    cursor: (usize, usize),
    attributes: &'static [(usize, usize, Color, Color, bool)],
}

const FG: Color = Color::rgb(204, 204, 204);
const BG: Color = Color::rgb(30, 30, 40);
const RED: Color = Color::rgb(205, 49, 49);
const BLUE: Color = Color::rgb(36, 114, 200);
const BRIGHT_RED: Color = Color::rgb(255, 109, 109);

#[test]
fn current_sequences_are_independent_of_chunking() {
    let rows = [
        Row { name: "cr lf bs", input: b"abc\rZ\n12\x08X", lines: ["Zbc     ", "1X      "], cursor: (2, 1), attributes: &[] },
        Row { name: "tab", input: b"a\tb", lines: ["a      b", "        "], cursor: (7, 0), attributes: &[] },
        Row { name: "cup and movement", input: b"\x1b[2;4HX\x1b[A\x1b[2D!\x1b[B\x1b[C?", lines: ["  !     ", "   X?   "], cursor: (5, 1), attributes: &[] },
        Row { name: "cha vpa", input: b"\x1b[2d\x1b[3Gz", lines: ["        ", "  z     "], cursor: (3, 1), attributes: &[] },
        Row { name: "erase line", input: b"abcdef\x1b[3G\x1b[K", lines: ["ab      ", "        "], cursor: (2, 0), attributes: &[] },
        Row { name: "erase display", input: b"abc\nxyz\x1b[1;2H\x1b[J", lines: ["a       ", "        "], cursor: (1, 0), attributes: &[] },
        Row { name: "erase preceding", input: b"abc\nxyz\x1b[2;2H\x1b[1K\x1b[1J", lines: ["        ", "  z     "], cursor: (1, 1), attributes: &[] },
        Row { name: "erase all", input: b"abc\x1b[2J", lines: ["        ", "        "], cursor: (0, 0), attributes: &[] },
        Row { name: "sgr", input: b"\x1b[1;31;44mA\x1b[22;91;49mB\x1b[0mC", lines: ["ABC     ", "        "], cursor: (3, 0), attributes: &[(0, 0, RED, BLUE, true), (1, 0, BRIGHT_RED, BG, false)] },
        Row { name: "sgr 30", input: b"\x1b[30mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(0, 0, 0) }, BG, false)] },
        Row { name: "sgr 90", input: b"\x1b[90mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(60, 60, 60) }, BG, false)] },
        Row { name: "sgr 31", input: b"\x1b[31mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(205, 49, 49) }, BG, false)] },
        Row { name: "sgr 91", input: b"\x1b[91mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(255, 109, 109) }, BG, false)] },
        Row { name: "sgr 32", input: b"\x1b[32mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(13, 188, 121) }, BG, false)] },
        Row { name: "sgr 92", input: b"\x1b[92mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(73, 248, 181) }, BG, false)] },
        Row { name: "sgr 33", input: b"\x1b[33mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(229, 229, 16) }, BG, false)] },
        Row { name: "sgr 93", input: b"\x1b[93mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(255, 255, 76) }, BG, false)] },
        Row { name: "sgr 34", input: b"\x1b[34mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(36, 114, 200) }, BG, false)] },
        Row { name: "sgr 94", input: b"\x1b[94mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(96, 174, 255) }, BG, false)] },
        Row { name: "sgr 35", input: b"\x1b[35mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(188, 63, 188) }, BG, false)] },
        Row { name: "sgr 95", input: b"\x1b[95mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(248, 123, 248) }, BG, false)] },
        Row { name: "sgr 36", input: b"\x1b[36mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(17, 168, 205) }, BG, false)] },
        Row { name: "sgr 96", input: b"\x1b[96mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(77, 228, 255) }, BG, false)] },
        Row { name: "sgr 37", input: b"\x1b[37mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(229, 229, 229) }, BG, false)] },
        Row { name: "sgr 97", input: b"\x1b[97mX", lines: ["X       ", "        "], cursor: (1, 0), attributes: &[(0, 0, const { Color::rgb(255, 255, 255) }, BG, false)] },
    ];
    for row in rows {
        for mode in 0..3 {
            let mut vt = TermEmu::new(8, 2);
            let mut pos = 0;
            let mut seed = 0x1234u32;
            while pos < row.input.len() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let n = match mode { 0 => row.input.len(), 1 => 1, _ => (seed as usize % 5) + 1 };
                let end = (pos + n).min(row.input.len());
                vt.feed(&row.input[pos..end]);
                pos = end;
            }
            assert_eq!(vt.cursor(), row.cursor, "{} mode {}", row.name, mode);
            for y in 0..2 {
                for (x, ch) in row.lines[y].chars().enumerate() {
                    let cell = vt.cell(x, y);
                    let attr = row.attributes.iter().find(|a| a.0 == x && a.1 == y)
                        .map(|a| (a.2, a.3, a.4)).unwrap_or((FG, BG, false));
                    assert_eq!((cell.ch, cell.fg, cell.bg, cell.bold), (ch, attr.0, attr.1, attr.2), "{} mode {} at {},{}", row.name, mode, x, y);
                }
            }
        }
    }
}

#[test]
fn characters_and_parser_bounds_are_independent_of_chunking() {
    let mut csi = b"\x1b[".to_vec();
    csi.extend(core::iter::repeat_n(b'0', 256));
    csi.extend_from_slice(b"m\x1b[31mX");
    let mut osc = b"\x1b]".to_vec();
    osc.extend(core::iter::repeat_n(b'x', 512));
    osc.extend_from_slice(b"\x07\x1b[31mX");
    let rows: Vec<(&str, Vec<u8>, &str, bool)> = vec![
        ("utf8_split", "Aé界B".as_bytes().to_vec(), "Aé界B", false),
        ("utf8_invalid", b"A\xe2\x82B\xffC".to_vec(), "A�B�C", false),
        ("combining", "a\u{301}界b".as_bytes().to_vec(), "a界b", false),
        ("16 parameters", b"\x1b[0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;31mX".to_vec(), "X", true),
        ("excess parameters", b"\x1b[0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;0;31mX".to_vec(), "X", false),
        ("long csi", csi, "X", true),
        ("long osc", osc, "X", true),
        ("can sub", b"\x1b[31\x18A\x1b]ignored\x1aB".to_vec(), "AB", false),
        ("unknown", b"\x1b[?31mA\x1b[1 qB\x1bZC".to_vec(), "ABC", false),
    ];
    for (name, input, expected, red) in rows {
        for mode in 0..3 {
            let mut vt = TermEmu::new(16, 2);
            let mut pos = 0;
            let mut seed = 0x1234u32;
            while pos < input.len() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let n = match mode { 0 => input.len(), 1 => 1, _ => (seed as usize % 5) + 1 };
                let end = (pos + n).min(input.len());
                vt.feed(&input[pos..end]);
                pos = end;
            }
            let actual: String = (0..expected.chars().count()).map(|x| vt.cell(x, 0).ch).collect();
            assert_eq!(actual, expected, "{name} mode {mode}");
            assert_eq!(vt.cursor(), (expected.chars().count(), 0), "{name} mode {mode}");
            assert_eq!(vt.cell(0, 0).fg, if red { RED } else { FG }, "{name} mode {mode}");
        }
    }
    for mode in 0..3 {
        let mut vt = TermEmu::new(10002, 1);
        let input = b"\x1b[999999999G";
        for chunk in input.chunks(match mode { 0 => input.len(), 1 => 1, _ => 3 }) { vt.feed(chunk); }
        assert_eq!(vt.cursor(), (9998, 0), "clamp mode {mode}");
    }
}


struct ScreenRow {
    name: &'static str,
    input: &'static [u8],
    lines: &'static [&'static str],
    cursor: (usize, usize),
    alt: bool,
    visible: bool,
    attributes: &'static [(usize, usize, Color, Color, bool)],
}

fn screen(name: &'static str, input: &'static [u8], lines: &'static [&'static str], cursor: (usize, usize)) -> ScreenRow {
    ScreenRow { name, input, lines, cursor, alt: false, visible: true, attributes: &[] }
}

#[test]
fn screen_editing_and_modes_are_independent_of_chunking() {
    let rows = [
        ScreenRow { alt: true, ..screen("alt_screen_1049", b"MAIN\x1b[2;3H\x1b[?1049h", &["        ", "        "], (2, 1)) },
        screen("1049 restores main and cursor", b"MAIN\x1b[2;3H\x1b[?1049hALT\x1b[?1049l!", &["MAIN    ", "  !     "], (3, 1)),
        ScreenRow { alt: true, ..screen("1049 reentry clears", b"MAIN\x1b[?1049hALT\x1b[?1049l\x1b[?1049h", &["        ", "        "], (4, 0)) },
        screen("1049 repeated set preserves saved main", b"M\x1b[?1049hA\x1b[?1049h\x1b[?1049l!", &["M!      ", "        "], (2, 0)),
        screen("IND bottom margin", b"A\r\nB\r\nC\r\nD\x1b[2;3r\x1b[3;2H\x1bDX", &["A       ", "C       ", " X      ", "D       "], (2, 2)),
        screen("RI top margin", b"A\r\nB\r\nC\r\nD\x1b[2;3r\x1b[2;2H\x1bMX", &["A       ", " X      ", "B       ", "D       "], (2, 1)),
        screen("LF bottom margin", b"A\r\nB\r\nC\r\nD\x1b[2;3r\x1b[3;2H\nX", &["A       ", "C       ", "X       ", "D       "], (1, 2)),
        screen("index outside region", b"A\r\nB\r\nC\r\nD\x1b[2;3r\x1b[4;2H\x1bDX", &["A       ", "B       ", "C       ", "DX      "], (2, 3)),
        screen("region reset", b"A\r\nB\r\nC\r\nD\x1b[2;3r\x1b[r\x1b[4;1H\x1bD", &["B       ", "C       ", "D       ", "        "], (0, 3)),
        screen("invalid region ignored", b"\x1b[2;3H\x1b[3;2rX", &["        ", "  X     ", "        ", "        "], (3, 1)),
        screen("insert line in region", b"A\r\nB\r\nC\r\nD\x1b[2;3r\x1b[2;2H\x1b[L", &["A       ", "        ", "B       ", "D       "], (0, 1)),
        screen("delete lines clamped", b"A\r\nB\r\nC\r\nD\x1b[2;3r\x1b[2;2H\x1b[999M", &["A       ", "        ", "        ", "D       "], (0, 1)),
        screen("delete line moves remainder", b"A\r\nB\r\nC\r\nD\x1b[2;3r\x1b[2;2H\x1b[M", &["A       ", "C       ", "        ", "D       "], (0, 1)),
        screen("line edits outside region ignored", b"A\r\nB\r\nC\r\nD\x1b[2;3r\x1b[4;2H\x1b[L\x1b[M", &["A       ", "B       ", "C       ", "D       "], (1, 3)),
        screen("insert characters", b"abcdef\x1b[3G\x1b[2@", &["ab  cdef", "        "], (2, 0)),
        screen("delete characters", b"abcdef\x1b[3G\x1b[2P", &["abef    ", "        "], (2, 0)),
        screen("erase characters clamped", b"abcdef\x1b[3G\x1b[999X", &["ab      ", "        "], (2, 0)),
        ScreenRow { attributes: &[(0, 0, RED, BG, false), (2, 0, RED, BG, false)], ..screen("editing moves attributes", b"\x1b[31mAB\x1b[0mC\x1b[2G\x1b[@", &["A BC    ", "        "], (1, 0)) },
        screen("pending wrap", b"12345678", &["12345678", "        "], (7, 0)),
        screen("wrap on next character", b"12345678X", &["12345678", "X       "], (1, 1)),
        screen("CR cancels pending wrap", b"12345678\rX", &["X2345678", "        "], (1, 0)),
        screen("CUP cancels pending wrap", b"12345678\x1b[1;8HX", &["1234567X", "        "], (7, 0)),
        screen("SGR preserves pending wrap", b"12345678\x1b[0mX", &["12345678", "X       "], (1, 1)),
        screen("autowrap off and on", b"\x1b[?7l12345678XY\x1b[?7hZ!", &["1234567Z", "!       "], (1, 1)),
        screen("default tab stops", b"a\tb\tc", &["a       b      c", "                "], (15, 0)),
        screen("HTS TBC current and all", b"\x1b[3g\x1b[4G\x1bH\r\tA\x1b[4G\x1b[g\r\tB", &["   A   B", "        "], (7, 0)),
        screen("tab does not erase", b"abcdefgh\r\tX", &["abcdefgX", "        "], (7, 0)),
        ScreenRow { attributes: &[(0, 0, RED, BLUE, true), (1, 0, RED, BLUE, true), (2, 0, RED, BLUE, true)], ..screen("DECSC DECRC rendition", b"\x1b[1;31;44mAB\x1b7\x1b[0m\x1b[2;1Hq\x1b8Z", &["ABZ     ", "q       "], (3, 0)) },
        screen("DECSC restores pending wrap", b"12345678\x1b7\r\x1b8X", &["12345678", "X       "], (1, 1)),
        ScreenRow { visible: false, ..screen("DECTCEM hide", b"A\x1b[?25l", &["A       ", "        "], (1, 0)) },
        screen("DECTCEM show", b"A\x1b[?25l\x1b[?25h", &["A       ", "        "], (1, 0)),
        screen("private parameters stay private", b"A\x1b[?2J\x1b[25l", &["A       ", "        "], (1, 0)),
    ];
    for row in rows {
        for mode in 0..3 {
            let cols = row.lines[0].chars().count();
            let mut vt = TermEmu::new(cols, row.lines.len());
            let mut pos = 0;
            let mut seed = 0x1234u32;
            while pos < row.input.len() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let n = match mode { 0 => row.input.len(), 1 => 1, _ => (seed as usize % 5) + 1 };
                let end = (pos + n).min(row.input.len());
                vt.feed(&row.input[pos..end]);
                pos = end;
            }
            assert_eq!(vt.alternate_screen(), row.alt, "{} mode {mode}", row.name);
            assert_eq!(vt.cursor_visible(), row.visible, "{} mode {mode}", row.name);
            assert_eq!(vt.cursor(), row.cursor, "{} mode {mode}", row.name);
            for (y, line) in row.lines.iter().enumerate() {
                assert_eq!(line.chars().count(), cols, "{} fixture width", row.name);
                for (x, ch) in line.chars().enumerate() {
                    let cell = vt.cell(x, y);
                    let attr = row.attributes.iter().find(|a| a.0 == x && a.1 == y)
                        .map(|a| (a.2, a.3, a.4)).unwrap_or((FG, BG, false));
                    assert_eq!((cell.ch, cell.fg, cell.bg, cell.bold), (ch, attr.0, attr.1, attr.2), "{} mode {mode} at {x},{y}", row.name);
                }
            }
        }
    }
}

#[test]
fn extended_color_is_independent_of_chunking() {
    let input = b"\x1b[38;5;196;48;5;25mA\x1b[38;2;1;2;3;48;2;4;5;6mB";
    for mode in 0..3 {
        let mut vt = TermEmu::new(8, 2);
        let mut pos = 0;
        let mut seed = 0x1234u32;
        while pos < input.len() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = match mode { 0 => input.len(), 1 => 1, _ => (seed as usize % 5) + 1 };
            let end = (pos + n).min(input.len());
            vt.feed(&input[pos..end]);
            pos = end;
        }
        assert_eq!((vt.cell(0, 0).fg, vt.cell(0, 0).bg), (Color::rgb(255, 0, 0), Color::rgb(0, 95, 175)), "mode {mode}");
        assert_eq!((vt.cell(1, 0).fg, vt.cell(1, 0).bg), (Color::rgb(1, 2, 3), Color::rgb(4, 5, 6)), "mode {mode}");
    }
}

#[test]
fn rendition_replies_and_titles_are_independent_of_chunking() {
    let cases: &[(&[u8], &[(bool, bool)], &[u8], &str)] = &[
        (b"\x1b[4mA\x1b[24mB", &[(true, false), (false, false)], b"", "Terminal"),
        (b"\x1b[7mA\x1b[27mB", &[(false, true), (false, false)], b"", "Terminal"),
        (b"\x1b[1;4;7;38;2;1;2;3mA\x1b[0mB", &[(true, true), (false, false)], b"", "Terminal"),
        (b"\x1b[5n\x1b[2;4H\x1b[6n\x1b[c", &[], b"\x1b[0n\x1b[2;4R\x1b[?1;2c", "Terminal"),
        (b"\x1b]0;hello\x07", &[], b"", "hello"),
        (b"\x1b]2;world\x1b\\", &[], b"", "world"),
        (b"\x1b]0;old\x07\x1b]2;new\x07", &[], b"", "new"),
    ];
    for &(input, attrs, reply, title) in cases {
        for mode in 0..3 {
            let mut vt = TermEmu::new(8, 2);
            let mut pos = 0;
            let mut seed = 0x1234u32;
            while pos < input.len() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let n = match mode { 0 => input.len(), 1 => 1, _ => (seed as usize % 5) + 1 };
                let end = (pos + n).min(input.len());
                vt.feed(&input[pos..end]);
                pos = end;
            }
            for (x, &(underline, inverse)) in attrs.iter().enumerate() {
                let cell = vt.cell(x, 0);
                assert_eq!((cell.underline, cell.inverse), (underline, inverse), "{input:?} mode {mode}");
            }
            assert_eq!(vt.replies.iter().copied().collect::<Vec<_>>(), reply, "{input:?} mode {mode}");
            assert_eq!(vt.title(), title, "{input:?} mode {mode}");
        }
    }
}
