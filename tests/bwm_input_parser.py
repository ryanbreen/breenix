#!/usr/bin/env python3
"""Compile the actual bwm stdin parser and exercise Escape disambiguation."""
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[1]
source = (root / 'userspace/programs/src/bwm.rs').read_text()
parser = source[source.index('const ESCAPE_TIMEOUT_MS:'):source.index('// ─── Hotkey System')]
modifiers = source[source.index('mod modifier {'):source.index('/// What a key press is to the chord bindings.')]
tests = r'''
fn key(event: Option<InputEvent>) -> (u8, u16, u16, Option<bool>) {
    match event.unwrap() {
        InputEvent::Key { ascii, keycode, modifiers, repeat } => (ascii, keycode, modifiers, repeat),
        InputEvent::FunctionKey(k) => panic!("unexpected function key {}", k),
    }
}
#[test]
fn escape_expires_before_a_later_unmodified_q() {
    let mut p = InputParser::new();
    assert!(p.feed(27, 100).is_none());
    assert!(p.expire_escape(129).is_none());
    assert_eq!(key(p.expire_escape(130)), (27, 27, 0, None));
    assert_eq!(key(p.feed(b'q', 5000)), (b'q', b'q' as u16, 0, None));
    assert!(p.expire_escape(5001).is_none());
}
#[test]
fn alt_and_split_sequences_still_decode_before_the_deadline() {
    let mut p = InputParser::new();
    assert!(p.feed(27, 100).is_none());
    assert!(p.expire_escape(110).is_none());
    assert_eq!(key(p.feed(b'q', 110)), (b'q', b'q' as u16, modifier::ALT as u16, None));
    for b in b"\x1b[13;9" { assert!(p.feed(*b, 200).is_none()); }
    assert!(p.expire_escape(300).is_none());
    assert_eq!(key(p.feed(b'u', 300)), (13, 0x28, modifier::SUPER as u16, Some(false)));
    assert!(p.feed(27, 400).is_none());
    assert!(p.feed(b'O', 400).is_none());
    assert!(matches!(p.feed(b'P', 400), Some(InputEvent::FunctionKey(1))));
}
'''
with tempfile.TemporaryDirectory(prefix='bwm-input-') as temp:
    path = Path(temp)
    (path / 'parser.rs').write_text(parser + modifiers + tests)
    subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '--test', str(path / 'parser.rs'), '-o', str(path / 'tests')], cwd=root, check=True)
    subprocess.run([str(path / 'tests')], check=True)
