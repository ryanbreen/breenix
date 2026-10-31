"""Exercise EVO publication against the documented one-update notifier scope."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class Notifier(unittest.TestCase):
    def test_update_disarms_notification_after_the_fresh_request(self):
        source = (ROOT / 'kernel/src/gpu/gk208/native.rs').read_text()
        code = item(source, '\nfn submit(')
        fixture = r'''
struct Registers { words: Vec<u32>, published: Option<u32> }
fn write_words(r: &mut Registers, _: u64, _: u64, words: &[u32]) -> Result<(), &'static str> {
    r.words.extend_from_slice(words); Ok(())
}
mod modeset {
    pub fn publish(r: &mut super::Registers, put: u32) -> Result<(), &'static str> {
        r.published = Some(put); Ok(())
    }
}
CODE
#[test] fn notification_is_scoped_to_each_update() {
    let mut r = Registers { words: vec![], published: None }; let mut put = 0;
    for _ in 0..2 {
        submit(&mut r, 4096, 0, &mut put, &[(0x84, 0x80000000), (0x80, 0)]).unwrap();
    }
    assert_eq!(r.words, [0x40084, 0x80000000, 0x40080, 0, 0x40084, 0,
        0x40084, 0x80000000, 0x40080, 0, 0x40084, 0]);
    assert_eq!(put, 48); assert_eq!(r.published, Some(48));
}
#[test] fn missing_update_does_not_create_one() {
    let mut r = Registers { words: vec![], published: None }; let mut put = 0;
    submit(&mut r, 4096, 0, &mut put, &[(0x84, 0x80000000)]).unwrap();
    assert_eq!(r.words, [0x40084, 0x80000000]); assert_eq!(put, 8);
}
#[test] fn the_trailing_disable_must_fit_before_publication() {
    let mut r = Registers { words: vec![], published: None }; let mut put = 2032;
    assert!(submit(&mut r, 4096, 0, &mut put, &[(0x84, 0x80000000), (0x80, 0)]).is_err());
    assert!(r.words.is_empty()); assert_eq!(r.published, None); assert_eq!(put, 2032);
}
'''.replace('CODE', code)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p / 'notifier.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(p / 'notifier.rs'), '-o', str(p / 'notifier')], check=True)
            subprocess.run([str(p / 'notifier'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
