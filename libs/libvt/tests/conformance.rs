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
