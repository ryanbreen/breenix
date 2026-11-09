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
        Row { name: "tab", input: b"a\tb", lines: ["a       ", "b       "], cursor: (1, 1), attributes: &[] },
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
                for (x, ch) in row.lines[y].bytes().enumerate() {
                    let cell = vt.cell(x, y);
                    let attr = row.attributes.iter().find(|a| a.0 == x && a.1 == y)
                        .map(|a| (a.2, a.3, a.4)).unwrap_or((FG, BG, false));
                    assert_eq!((cell.ch, cell.fg, cell.bg, cell.bold), (ch, attr.0, attr.1, attr.2), "{} mode {} at {},{}", row.name, mode, x, y);
                }
            }
        }
    }
}
