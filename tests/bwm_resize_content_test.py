"""Client pixels remain usable beside the window manager's resize border."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class ResizeContent(unittest.TestCase):
    def test_resize_grab_scales_from_logical_units(self):
        config = (ROOT / "userspace/programs/src/bwm/config.rs").read_text()
        desk = (ROOT / "userspace/programs/src/bwm/desk.rs").read_text()
        import re
        self.assertRegex(config, r"const RESIZE_GRAB: usize = 8;")
        self.assertRegex(item(desk, "pub(super) fn hit_resize_edge("), r"px\(RESIZE_GRAB\)")
        self.assertRegex(item(config, "pub(super) fn px("), r"n\s*\*\s*ui\(\)\.scale")
        self.assertEqual(8 * 2, 16)

    def test_content_edges_deliver_to_client_and_frame_still_resizes(self):
        source = (ROOT / "userspace/programs/src/bwm/desk.rs").read_text()
        methods = "\n".join(item(source, f"pub(super) fn {name}(") for name in [
            "content_x", "content_y", "content_width", "content_height",
            "total_height", "hit_content", "hit_resize_edge",
        ]).replace("pub(super)", "")
        run_rust(self, r'''
const BORDER_WIDTH: usize = 2;
const TITLE_BAR_HEIGHT: usize = 32;
const RESIZE_GRAB: usize = 8;
thread_local! { static SCALE: std::cell::Cell<usize> = const { std::cell::Cell::new(1) }; }
fn px(n: usize) -> usize { SCALE.with(|s| n * s.get()) }
#[derive(Debug, PartialEq)]
enum ResizeEdge { TopLeft, TopRight, BottomLeft, BottomRight, Left, Right, Top, Bottom }
struct Window { x: i32, y: i32, width: usize, height: usize, chromeless: bool }
impl Window { METHODS }
#[test]
fn client_edges_and_frame() {
    for scale in [1, 2] {
        SCALE.with(|s| s.set(scale));
        let mut w = Window { x: 100, y: 80, width: 800 + px(4), height: 600 + px(36), chromeless: false };
        let (x, y) = (w.content_x(), w.content_y());
        // Actual Gus Kit stroke (10, 176+10), plus every client corner.
        for (dx, dy) in [(10, 186), (0, 0), (799, 0), (0, 599), (799, 599)] {
            assert!(w.hit_content(x + dx, y + dy));
            assert_eq!(w.hit_resize_edge(x + dx, y + dy), None, "scale={scale} client=({dx},{dy})");
        }
        assert_eq!(w.hit_resize_edge(x - 1, y + 80), Some(ResizeEdge::Left));
        assert_eq!(w.hit_resize_edge(x + 800, y + 80), Some(ResizeEdge::Right));
        assert_eq!(w.hit_resize_edge(x + 80, y + 600), Some(ResizeEdge::Bottom));
        assert_eq!(w.hit_resize_edge(w.x, w.y), Some(ResizeEdge::TopLeft));
        w.chromeless = true;
        assert_eq!(w.hit_resize_edge(w.x, w.y), None);
    }
}
'''.replace("METHODS", methods))


if __name__ == "__main__":
    unittest.main()
