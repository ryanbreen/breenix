//! The CPU reference against the contract's literal tables (docs/gui/gpu.md section 2.4):
//! section 5.3's line walks and section 5.5's midpoint-circle octants, read from
//! docs/gui/breengel.md itself so no transcription stands between them.

use std::collections::HashSet;

use libgfx_ref::color::Color;
use libgfx_ref::framebuf::{Format, FrameBuf};
use libgfx_ref::shapes;

static CONTRACT: &str = include_str!("../../../docs/gui/breengel.md");
const INK: Color = Color::rgb(200, 30, 90);

/// The contract's section between `start` and the next heading after it.
fn section(start: &str) -> &'static str {
    let from = CONTRACT.find(start).unwrap_or_else(|| panic!("the contract has no {start}"));
    let rest = &CONTRACT[from + start.len()..];
    &rest[..rest.find("\n### ").unwrap_or(rest.len())]
}

/// The table rows (`| a | b |`) of `text` after its header and separator, as cells.
fn rows(text: &str) -> Vec<Vec<&str>> {
    text.lines()
        .filter(|l| l.starts_with('|') && !l.starts_with("|---"))
        .skip(1)
        .map(|l| l.trim_matches('|').split('|').map(str::trim).collect())
        .collect()
}

fn inked(fb: &FrameBuf) -> HashSet<(i64, i64)> {
    (0..fb.height).flat_map(|y| (0..fb.width).map(move |x| (x, y)))
        .filter(|&(x, y)| fb.get_pixel(x, y) == INK)
        .map(|(x, y)| (x as i64, y as i64))
        .collect()
}

#[test]
fn lines_walk_the_contract_table() {
    let table = rows(section("### 5.3 `draw_line"));
    assert_eq!(table.len(), 8, "section 5.3 lists eight lines");
    for row in table {
        let ends: Vec<i32> = row[0].trim_matches('`').split(['→', ',']).map(|v| v.trim().parse().unwrap()).collect();
        let want: HashSet<(i64, i64)> = row[1].split(')').filter(|p| p.contains('('))
            .map(|p| {
                let (x, y) = p.trim().trim_start_matches('(').split_once(',').unwrap();
                (x.parse::<i64>().unwrap() + 3, y.parse::<i64>().unwrap() + 3)
            })
            .collect();
        let mut fb = FrameBuf::new(20, 20, Format::BGRX).unwrap();
        shapes::draw_line(&mut fb, ends[0] + 3, ends[1] + 3, ends[2] + 3, ends[3] + 3, INK);
        assert_eq!(inked(&fb), want, "line {}", row[0]);
    }
}

#[test]
fn circles_match_the_contract_octants() {
    let table = rows(section("### 5.5 `draw_circle"));
    assert_eq!(table.len(), 41, "section 5.5 lists radii 0 to 40");
    for row in table {
        let r: i32 = row[0].parse().unwrap();
        let mut want = HashSet::new();
        for (y, x) in row[1].split_whitespace().map(|x| x.parse::<i64>().unwrap()).enumerate() {
            let y = y as i64;
            for (a, b) in [(x, y), (y, x)] {
                for (sa, sb) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
                    want.insert((45 + sa * a, 45 + sb * b));
                }
            }
        }
        let mut fb = FrameBuf::new(91, 91, Format::BGRX).unwrap();
        shapes::draw_circle(&mut fb, 45, 45, r, INK);
        assert_eq!(inked(&fb), want, "radius {r}");
    }
}
