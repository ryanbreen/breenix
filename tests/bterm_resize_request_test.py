"""Execute the suite's resize request against live-size window fixtures."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ResizeRequest(unittest.TestCase):
    def test_request_grows_the_current_window_in_logical_units(self):
        source = (ROOT / "userspace/programs/src/guiapps_suite/bterm.rs").read_text()
        case = source.split("pub fn resize() -> CaseResult {", 1)[1]
        request = case.split("    // App::resize drags", 1)[1]
        request = request[request.index("\n"):request.index('    until("resized terminal grid"')]
        code = r'''
use std::cell::Cell;
thread_local! { static CURRENT: Cell<(u32,u32,u32)> = const { Cell::new((0,0,0)) }; }
pub mod libbreenix {
    pub mod gpu {
        pub struct Mode { pub scale: u32 }
        pub fn current_mode() -> Result<Mode, String> {
            Ok(Mode { scale: crate::CURRENT.get().2 })
        }
    }
    pub mod window {
        pub mod abi {
            #[derive(Clone, Copy, Default)]
            pub struct WinInfo { pub id: u64, pub width: u32, pub height: u32 }
        }
        pub fn list(out: &mut [abi::WinInfo]) -> Result<usize, String> {
            let (width,height,_) = crate::CURRENT.get();
            out[0] = abi::WinInfo { id: 11, width: 400, height: 300 };
            out[1] = abi::WinInfo { id: 22, width, height };
            Ok(2)
        }
    }
}
pub struct App;
impl App {
    pub fn window(&self) -> u64 { 22 }
    fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        let (current_w,current_h,scale) = CURRENT.get();
        assert!(width > current_w && height > current_h,
                "scale {scale}: request {width}x{height} must grow {current_w}x{current_h}");
        assert_eq!((width-current_w,height-current_h),(80*scale,60*scale));
        println!("scale {scale}: {current_w}x{current_h} -> {width}x{height}");
        Ok(())
    }
}
fn request(t: &mut App) -> Result<(), String> {
REQUEST
    Ok(())
}
fn main() {
    for scale in [2,1] {
        for (w,h) in [(750,550),(640,480)] {
            CURRENT.set((w*scale,h*scale,scale));
            request(&mut App).unwrap();
        }
    }
}
'''.replace("REQUEST", request)
        with tempfile.TemporaryDirectory(prefix="bterm-resize-") as directory:
            path = Path(directory)
            (path / "main.rs").write_text(code)
            priority = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
            subprocess.run(priority + ["rustc", "--edition=2021", "-Dwarnings",
                           str(path / "main.rs"), "-o", str(path / "check")],
                           check=True, env=dict(os.environ, CARGO_BUILD_JOBS="4"))
            subprocess.run([str(path / "check")], check=True)


if __name__ == "__main__":
    unittest.main()
