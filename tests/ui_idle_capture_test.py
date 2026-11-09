"""Exercise the production screen predicate and controller socket configuration."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(path, signature):
    source = (ROOT / path).read_text()
    rest = source[source.index(signature):]
    end = rest.index('{') + 1
    depth = 1
    while depth:
        depth += (rest[end] == '{') - (rest[end] == '}')
        end += 1
    return rest[:end]


def run_rust(source):
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / 'check.rs').write_text(source)
        build = subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                                str(root / 'check.rs'), '-o', str(root / 'check')],
                               capture_output=True, text=True)
        if build.returncode:
            raise AssertionError(build.stderr)
        return subprocess.run([str(root / 'check')], capture_output=True, text=True)


class IdleCapture(unittest.TestCase):
    def test_unchanged_visible_window_still_binds_to_observation(self):
        front = function('userspace/programs/src/uitest_suite/proof_wm_checks.rs', 'pub fn front_matches(')
        binding = function('userspace/programs/src/uitest_suite/proof.rs', 'fn wm_front_matches(')
        source = '''mod libbreenix { pub mod window { pub mod abi {
            pub struct CompositeEntry { pub commit_id:u64, pub pixel_gen:u64, pub pixels:u64 }
        } } }
        mod wm_checks {''' + front + '}\n' + binding + '''
        fn main() {
            use libbreenix::window::abi::CompositeEntry;
            // scene::finish records work in this update, not pixels still visible.
            let unchanged = CompositeEntry {commit_id:20, pixel_gen:7, pixels:0};
            assert_eq!(unchanged.pixels, 0);
            assert!(wm_front_matches(19,7,7,Some(20),&unchanged));
            assert!(!wm_front_matches(19,7,7,None,&unchanged));
            assert!(!wm_front_matches(19,7,7,Some(19),&unchanged));
            assert!(!wm_front_matches(19,7,8,Some(20),&unchanged));
            assert!(!wm_front_matches(19,6,6,Some(20),&unchanged));
        }'''
        result = run_rust(source)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_capture_producer_waits_for_controller_without_an_idle_deadline(self):
        source = (ROOT / 'userspace/programs/src/uitest_capture_app.rs').read_text()
        start = source.index('    socket.set_read_timeout(')
        end = source.index('    let mut socket = BufReader::new(socket);', start)
        setup = source[start:end]
        result = run_rust('''use std::{os::unix::net::UnixStream,time::Duration};
        fn configure(socket:&UnixStream)->Result<(),String> {''' + setup + '''Ok(())}
        fn main() {
            let (socket, _peer) = UnixStream::pair().unwrap();
            configure(&socket).unwrap();
            assert_eq!(socket.read_timeout().unwrap(), None,
                "the runner owns the case deadline while the app awaits a command");
            assert_eq!(socket.write_timeout().unwrap(), Some(Duration::from_secs(2)));
        }''')
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
