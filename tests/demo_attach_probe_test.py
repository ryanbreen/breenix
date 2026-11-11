"""Exercise real demo dispatch callbacks after the libuitest ui_app host build."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def callback(name):
    text = (ROOT / f'userspace/programs/src/{name}.rs').read_text()
    start = text.index('|delivered| {' if name == 'bounce' else '|d| {')
    start = text.index('{', start) + 1
    depth, end = 1, start
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[start:end - 1]


class AttachProbe(unittest.TestCase):
    def test_motion_publishes_a_tree_without_advancing_simulation(self):
        host = subprocess.check_output(['rustc', '-vV'], text=True).split('host: ')[1].splitlines()[0]
        deps = ROOT / 'libs/libuitest/target' / host / 'debug/deps'
        breengel = max(deps.glob('libbreengel-*.rlib'), key=lambda p: p.stat().st_mtime)
        setups = {
            'bounce': '''let mut simulation=bounce_sim::Simulation::new(400,300);
let before=simulation.spheres.clone(); let mut last_tick=None; let mut fps=Fps; let mut handled_input=false;''',
            'particles': '''let mut sim=particle_sim::Simulation::new(400,300);
let before=sim.particles.clone(); let mut last=None; let mut input=false;''',
            'demo': '''let mut last=None; let mut remainder=0u64; let mut epochs=0u64; let mut frame=0;
let mut balls:[Ball;0]=[]; let (win_width,win_height)=(400,300); let mut input=false;''',
            'rectangles': '''let mut last=None; let mut remainder=0u64; let mut epochs=0u64;
let mut rects:[Rect;0]=[]; let (width,height)=(400,300); let mut input=false;''',
        }
        observations = {
            'bounce': 'assert!(handled_input); assert_eq!(simulation.spheres,before); assert_eq!(simulation.epochs(),0); assert_eq!(last_tick,None);',
            'particles': 'assert!(input); assert_eq!(sim.particles,before); assert_eq!(sim.epochs(),0); assert_eq!(last,None);',
            'demo': 'assert!(input); assert_eq!((last,remainder,epochs,frame),(None,0,0,0));',
            'rectangles': 'assert!(input); assert_eq!((last,remainder,epochs),(None,0,0));',
        }
        source = f'''use breengel::{{Event,Handled,Delivered}};
#[path="{ROOT}/userspace/programs/src/bounce/sim.rs"] pub mod bounce_sim;
#[path="{ROOT}/userspace/programs/src/particles/sim.rs"] pub mod particle_sim;
#[path="{ROOT}/libs/breengel/src/tree_commit.rs"] mod tree_commit;
struct Fps; impl Fps {{ fn tick(&mut self,_now:u64) {{ panic!("attach motion ran a clock callback") }} }}
struct Ball; impl Ball {{ fn update(&mut self,_w:i32,_h:i32) {{ panic!("attach motion moved a ball") }} }}
struct Rect {{x:i32,y:i32,vx:i32,vy:i32}}
impl Rect {{ fn clamp(&mut self,_w:i32,_h:i32) {{ panic!("attach motion moved a rectangle") }} }}
'''
        for name, setup in setups.items():
            binding = 'delivered' if name == 'bounce' else 'd'
            source += f'''#[test] fn {name}_attach_motion() {{
let mut redraw=false; let mut close=false; let mut resize=None;
{setup}
let disposition=(|{binding}: &Delivered| {{ {callback(name)} }})(&Delivered {{serial:1,time_ns:0,clicks:0,event:Event::MouseMove{{x:264,y:294}}}});
assert_eq!(disposition,Handled::Redraw,"{name}: attach probe needs a pixel/tree commit; metadata-only acknowledgments retain the pre-attach tree");
let mut serializations=0;
if redraw {{ tree_commit::serialize_for_commit(1,&mut serializations,||Ok(())).unwrap(); }}
assert_eq!(serializations,1); assert!(!close); assert_eq!(resize,None);
{observations[name]}
}}
'''
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch)
            (path / 'probe.rs').write_text(source)
            result = subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                                     str(path / 'probe.rs'), '-L', f'dependency={deps}',
                                     '--extern', f'breengel={breengel}', '-o', str(path / 'probe')],
                                    capture_output=True, text=True, env=dict(os.environ, CARGO_BUILD_JOBS='4'))
            self.assertEqual(result.returncode, 0, result.stderr)
            result = subprocess.run([str(path / 'probe')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
