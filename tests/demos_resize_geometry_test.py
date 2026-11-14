"""Keep Icon Kit's demo resize gesture on screen when bwm discovers it early.

Execute the app startup, bwm placement/hit testing and App::resize coordinates.
The window boundary forces discovery at creation, before any startup resize.
"""
from pathlib import Path
import os
import re
import subprocess
import unittest

from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


def source(path):
    revision = os.environ.get('DEMOS_RESIZE_BASELINE')
    if revision:
        return subprocess.check_output(['git', 'show', f'{revision}:{path}'],
                                       cwd=ROOT, text=True)
    return (ROOT / path).read_text()


class DemosResizeGeometry(unittest.TestCase):
    def test_icon_kit_resize_edges_stay_reachable(self):
        app = source('userspace/programs/src/biconkit.rs')
        startup = app[app.index('fn main() {') + len('fn main() {'):app.index('    let mut grid=')]
        config = source('userspace/programs/src/bwm/config.rs')
        constants = '\n'.join(re.findall(
            r'^pub\(super\) const (?:BORDER_WIDTH|TITLE_BAR_HEIGHT|RESIZE_GRAB|'
            r'TASKBAR_HEIGHT|APPBAR_HEIGHT):[^\n]+', config, re.M)).replace('pub(super)', '')
        desk = source('userspace/programs/src/bwm/desk.rs')
        methods = '\n'.join(item(desk, f'pub(super) fn {name}(') for name in (
            'content_x', 'content_y', 'content_width', 'content_height',
            'total_height', 'hit_content', 'hit_resize_edge')).replace('pub(super)', '')
        resize = item(source('libs/libuitest/src/app.rs'), 'pub fn resize(')
        point = resize[resize.index('                let from ='):resize.index('                live.move_to')]
        target = re.search(r'let \(width, height\) = [^;]+;',
                           item(source('userspace/programs/src/guiapps_suite/demos.rs'),
                                'pub fn resize(')).group()
        placement = source('userspace/programs/src/bwm/placement.rs')
        run_rust(self, r'''
CONSTANTS
PLACEMENT
thread_local! { static SCALE: std::cell::Cell<usize> = const { std::cell::Cell::new(1) }; }
fn px(n: usize) -> usize { SCALE.with(|s| n * s.get()) }
#[derive(Debug, PartialEq)]
enum ResizeEdge { TopLeft, TopRight, BottomLeft, BottomRight, Left, Right, Top, Bottom }
struct Frame { x: i32, y: i32, width: usize, height: usize, chromeless: bool }
impl Frame { METHODS }
pub mod libbreenix { pub mod gpu {
    pub struct Mode { pub scale: u32 }
    pub fn current_mode() -> Result<Mode, String> { Ok(Mode { scale: super::super::px(1) as u32 }) }
} }
pub struct Surface;
impl Surface { pub fn ui_scale(&self) -> u32 { px(1) as u32 } }
pub struct Window { frame: Frame, width: u32, height: u32, surface: Surface }
impl Window {
    pub fn new_without_fonts(_: &[u8], width: u32, height: u32) -> Result<Self, String> {
        let (fw, fh) = (width as usize + px(BORDER_WIDTH) * 2,
                       height as usize + px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2);
        // demos::each retains the first five demo windows before launching Icon Kit.
        let (x, y) = cascade_position([0, px(TASKBAR_HEIGHT), 2560,
            1600 - px(TASKBAR_HEIGHT) - px(APPBAR_HEIGHT)], [fw, fh], px(1), 5);
        Ok(Self { frame: Frame { x, y, width: fw, height: fh, chromeless: false },
                  width, height, surface: Surface })
    }
    pub fn surface(&mut self) -> Result<&mut Surface, String> { Ok(&mut self.surface) }
    pub fn apply_resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        // discover_windows updates dimensions in place without moving an existing frame.
        self.width = width; self.height = height;
        self.frame.width = width as usize + px(BORDER_WIDTH) * 2;
        self.frame.height = height as usize + px(TITLE_BAR_HEIGHT) + px(BORDER_WIDTH) * 2;
        Ok(())
    }
}
struct Info { x: i32, y: i32, width: u32, height: u32 }
#[test]
fn resize_gesture() {
    for scale in [1, 2] {
        SCALE.with(|s| s.set(scale as usize));
        STARTUP
        assert_eq!((win.width, win.height), (600 * scale, 480 * scale));
        TARGET
        assert_eq!((width, height), (300 * scale, 200 * scale));
        for bottom in [false, true] {
            let info = Info { x: win.frame.content_x(), y: win.frame.content_y(),
                              width: win.width, height: win.height };
            POINT
            let expected = if bottom { ResizeEdge::Bottom } else { ResizeEdge::Right };
            assert_eq!(win.frame.hit_resize_edge(from.0, from.1), Some(expected),
                       "scale={scale} bottom={bottom} point={from:?}");
            assert!(from.0 >= 0 && from.0 < 2560 && from.1 >= 0 && from.1 < 1600,
                    "scale={scale} bottom={bottom} drag point {from:?} outside 2560x1600");
            // App::resize completes width before starting its bottom-edge drag.
            if !bottom { win.apply_resize(width, win.height).unwrap(); }
        }
    }
}
'''.replace('CONSTANTS', constants).replace('PLACEMENT', placement)
            .replace('METHODS', methods).replace('STARTUP', startup)
            .replace('TARGET', target).replace('POINT', point))


if __name__ == '__main__':
    unittest.main()
