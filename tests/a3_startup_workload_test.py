"""Census the retained replacement's glyph workload, without a guest or GPU.

The real parser, outline/layout/crossing builder, startup coverage helper and
atlas class/charge functions run on the host. Page growth is counted from the
slot allocator's doubling rule and compared with the captured device creates.
This is a workload guard, not a timing test or a reproduction of GPU waiting.
"""
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
CAPTURE = ROOT / "docs/planning/arm64-catchup/serials/a3-startup-216124b3.json"


class StartupWorkload(unittest.TestCase):
    def test_retained_cold_font_workload(self):
        font = ROOT / "fonts/DejaVuSans.ttf"
        self.assertEqual(hashlib.sha256(font.read_bytes()).hexdigest(),
                         "7da195a74c55bef988d0d48f9508bd5d849425c1770dba5d7bfc6ce9ed848954")
        source = (ROOT / "libs/libgfx/src/ttf_font.rs").read_text()
        code = r'''
use std::collections::{BTreeMap, BTreeSet};
use libfont::{Font, GlyphLayout, crossings::{lcd_segments, CrossingTable}};
#[path="@HELPER@"] pub mod startup_prepare;
@CLASS@
@CHARGE@
fn main() {
    let font = Font::parse(include_bytes!("@FONT@")).unwrap();
    // The clock alphabet in coverage makes this census independent of its digits.
    let texts = startup_prepare::coverage(b"00:00:00 ET", &[b"WM client 11"]);
    let mut seen = BTreeSet::new();
    let mut classes = BTreeMap::new();
    let (mut outlines, mut segments_total, mut table_bytes, mut crossings) = (0, 0, 0, 0);
    for text in texts {
        for ch in std::str::from_utf8(text).unwrap().chars() {
            let glyph = font.glyph_index(ch);
            if !seen.insert(glyph) { continue; }
            let outline = font.glyph_outline(glyph).unwrap();
            if let GlyphLayout::Outline { segments, width, height, .. } =
                font.layout_glyph(&outline, glyph, Font::size26(28.0).unwrap()).unwrap() {
                outlines += 1;
                segments_total += segments.len();
                let table = CrossingTable::new(&lcd_segments(&segments), 0, height);
                table_bytes += table.bytes().len();
                crossings += table.entries();
                *classes.entry(class(width, height)).or_insert(0usize) += 1;
            }
        }
    }
    println!("glyphs={} outlines={} segments={} crossings={} table_bytes={}",
             seen.len(), outlines, segments_total, crossings, table_bytes);
    for ((pitch, rows), count) in classes {
        println!("class pitch={} rows={} glyphs={}", pitch, rows, count);
        let mut held = 0usize;
        // ttf_font::slot grows 1, 1, 2, 4... slots of each class; these small
        // glyphs do not reach MAX_PAGE_ROWS or CachedFont::new's byte ceiling.
        while held < count {
            let slots = held.max(1);
            println!("page width={} height={} bytes={}", pitch / 4, rows * slots,
                     charge(pitch / 4, rows * slots));
            held += slots;
        }
    }
}
'''.replace("@HELPER@", str(ROOT / "userspace/programs/src/bwm/startup_prepare.rs"))
        code = code.replace("@FONT@", str(font)).replace("@CLASS@", item(source, "fn class("))
        code = code.replace("@CHARGE@", item(source, "fn charge("))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "census.rs").write_text(code)
            env = dict(os.environ, CARGO_BUILD_JOBS="4")
            def compile_rust(args):
                command = ["rustc", "--edition=2021", "-O", "-Dwarnings", *args]
                if sys.platform == "darwin":
                    command = ["taskpolicy", "-b", *command]
                result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            compile_rust(["--crate-type=rlib", "--crate-name=libfont",
                          "libs/libfont/src/lib.rs", "-o", str(path / "libfont.rlib")])
            compile_rust([str(path / "census.rs"), "--extern", f"libfont={path / 'libfont.rlib'}",
                          "-o", str(path / "census")])
            output = subprocess.check_output([str(path / "census")], text=True)
        print(output, end="", flush=True)
        totals = dict(re.findall(r"(\w+)=(\d+)", output.splitlines()[0]))
        self.assertEqual((int(totals["glyphs"]), int(totals["outlines"])), (26, 25))
        planned = Counter((int(w), int(h)) for w, h in
                          re.findall(r"page width=(\d+) height=(\d+)", output))
        capture = json.loads(CAPTURE.read_text())
        # Atlas pages (64-wide), separately from LCD scratch (256-wide),
        # full-screen targets and the context's internal 1x1 scratch.
        traced = Counter()
        for row in capture["host_resources"]:
            match = re.search(r"fmt 0x1, w (\d+), h (\d+),", row["text"])
            if match and int(match[1]) == 64:
                traced[(int(match[1]), int(match[2]))] += 1
        self.assertEqual(planned, traced)
        phases = {re.search(r"phase=(\S+)", r["text"])[1]:
                  int(re.search(r" ns=(\d+)", r["text"])[1]) for r in capture["phases"]}
        self.assertLess(phases["font-plan-begin"], phases["targets-ready"])
        self.assertLess(phases["targets-ready"], phases["font-plan-return"])
        first_ms = (phases["first-composite"] - phases["compositor-exec"]) / 1e6
        print(f"retained first composite={first_ms:.6f} ms; margin={2000-first_ms:.6f} ms")


if __name__ == "__main__":
    unittest.main()
