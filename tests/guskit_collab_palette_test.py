"""Execute the collaboration stroke helper against Gus Kit's scaled palette.

The host driver replaces input transport and collaboration I/O. Gus Kit's
render/publication, pointer mapping and HSV conversion use production code.
"""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def source(path):
    revision = os.environ.get('GUSKIT_COLLAB_BASELINE')
    if revision:
        return subprocess.check_output(['git', 'show', f'{revision}:{path}'], cwd=ROOT, text=True)
    return (ROOT / path).read_text()


class CollabPalette(unittest.TestCase):
    def test_stroke_selects_the_expected_colour_at_both_scales(self):
        app = source('userspace/programs/src/guskit.rs')
        code = (ROOT / 'tests/fixtures/guskit_layout.rs').read_text()
        code = code.replace('// LAYOUT_MODULE', 'mod layout {\n' + source('userspace/programs/src/guskit/layout.rs') + '\n}\nuse layout::Layout;')
        code = code.replace('// APP_LAYOUT_FIELD', 'layout: Layout,')
        code = code.replace('// APP_LAYOUT_INIT', 'layout: Layout::new(scale),')
        code = code.replace('// APP_METHODS', '\n'.join(item(app, f'    fn {name}(') for name in ['point', 'press']))
        code = code.replace('// PRODUCTION', '\n'.join(item(app, f'fn {name}(') for name in ['draw_label', 'render', 'publish']))
        code = code.replace('// NODE_MODULE', source('userspace/programs/src/guskit/node.rs'))
        code = code.replace('// CONSTANTS', '\n'.join(line for line in app.splitlines() if line.startswith(('const NAMES:', 'const LABELS:'))))
        scenario = r'''
mod collab_palette {
    use libbui::tree::Tree;
    struct App { model: super::App, published: Tree, stroke: Option<((i32,i32),Vec<(i32,i32)>)> }
    impl App {
        fn attach(&mut self, title: &str) -> Result<(),String> { assert_eq!(title,"Gus Kit (Peer)"); Ok(()) }
        fn key(&mut self, key: u16, modifiers: &[()]) -> Result<(),String> {
            assert_eq!(key,38); assert!(modifiers.is_empty()); self.model.tool=2; Ok(())
        }
        fn tree(&mut self) -> Result<Tree,String> { Ok(self.published.clone()) }
        fn drag(&mut self, key:&str, from:(i32,i32), steps:&[(i32,i32)]) -> Result<(),String> {
            let node=self.published.children.iter().find(|n|n.key==key).ok_or("missing node")?;
            let (x,y)=drag_start(node,from)?;
            self.model.press(x,y);
            if key=="canvas" { self.stroke=Some((from,steps.to_vec())); }
            Ok(())
        }
        fn wait_node(&mut self,key:&str,predicate:impl FnOnce(&serde_json::Value)->bool)->Result<(),String> {
            assert_eq!(key,"status");
            let value=serde_json::json!({"tool":super::NAMES[self.model.tool],"color":self.model.color()});
            if predicate(&value) { Ok(()) } else { Err(format!("status wait cannot pass: {value}")) }
        }
    }
    DRAG_START
    fn attach(t:&mut App,name:&str)->Result<(),String> { t.attach(&format!("Gus Kit ({name})")) }
    enum Op { Stroke { y:usize, color:[u8;3] } }
    struct Script { recorded:Option<(&'static str,Op)> }
    impl Script {
        fn record(&mut self,owner:&'static str,op:Op,refusal:Option<&str>) {
            assert_eq!(refusal,None); self.recorded=Some((owner,op));
        }
        STROKE
    }
    #[test]
    fn collab_palette_colour() {
        for scale in [1,2] { for green in [false,true] {
            let (mut model,_,published)=super::sample(scale,false);
            model.hue=0;model.sat=0;model.val=0;model.selected_color=None;model.stroke=None;
            let mut t=App {model,published,stroke:None};
            let mut script=Script {recorded:None};
            script.stroke(&mut t,"Peer",20,green).unwrap_or_else(|e|panic!("scale={scale} green={green}: {e}"));
            assert_eq!(t.stroke,Some(((10,20),vec![(30,0)])),"canvas strokes remain in image pixels");
            let Some((owner,Op::Stroke { y,color }))=script.recorded else {panic!("missing stroke")};
            assert_eq!((owner,y,color),("Peer",20,if green {[40,204,40]} else {[0,0,0]}));
        }}
    }
}
'''
        scenario = scenario.replace('DRAG_START', item(source('libs/libuitest/src/app.rs'), 'pub fn drag_start('))
        scenario = scenario.replace('STROKE', item(source('userspace/programs/src/guiapps_suite/guskit_collab.rs'), '    fn stroke('))
        code += scenario
        with tempfile.TemporaryDirectory(prefix='guskit-collab-palette-') as directory:
            path = Path(directory)
            (path / 'src').mkdir()
            (path / 'src/lib.rs').write_text(code)
            (path / 'Cargo.toml').write_text(f'''[package]
name = "guskit-collab-palette-check"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
libbui = {{ path = "{ROOT / 'libs/libbui'}" }}
libgfx = {{ path = "{ROOT / 'libs/libgfx'}" }}
serde_json = "=1.0.154"
''')
            env = dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings',
                       CARGO_TARGET_DIR=str(ROOT / 'tools/ui-host-tests/target'))
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            result = subprocess.run(priority + ['cargo', 'test', '--offline', '--manifest-path', str(path / 'Cargo.toml'),
                                               '--lib', 'collab_palette_colour', '--', '--nocapture'], env=env)
            self.assertEqual(result.returncode, 0)


if __name__ == '__main__':
    unittest.main()
