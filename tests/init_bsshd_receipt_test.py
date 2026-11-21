"""Score init's forwarded bsshd receipt after an unfinished getty prompt."""
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = runpy.run_path(str(ROOT / 'scripts/vm-artifacts.py'))


class BsshdReceipt(unittest.TestCase):
    def test_cutover_finds_readiness_after_getty_prompt(self):
        init = (ROOT / 'userspace/programs/src/init_services.rs').read_text()
        getty = (ROOT / 'userspace/programs/src/getty.rs').read_text()
        prompt = getty.split('let prompt = ', 1)[1].split(';', 1)[0]
        forward = item(item(init, 'fn drain_logs('), '|line| {')
        desktop_filter = item(init, 'fn desktop_log_receipt(')
        harness = desktop_filter + f'''
use std::io::Write;

struct Entry {{ def: Definition }}
struct Definition {{ name: &'static str }}
struct Output {{ stream: &'static str }}

fn observe(line: &[u8], forward: impl FnOnce(&[u8])) {{ forward(line); }}

fn main() {{
    println!("[boot] breenfs root filesystem mounted");
    std::io::stdout().write_all({prompt}).unwrap();
    std::io::stdout().flush().unwrap();
    let entry = Entry {{ def: Definition {{ name: "bsshd" }} }};
    let output = Output {{ stream: "stdout" }};
    observe(b"bsshd: listening on 0.0.0.0:22", {forward});
}}
'''
        with tempfile.TemporaryDirectory() as name:
            run = Path(name)
            source, binary = run / 'forward.rs', run / 'forward'
            source.write_text(harness)
            background = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run([*background, 'rustc', '--edition=2021', '-Dwarnings',
                            source, '-o', binary], check=True)
            serial = subprocess.check_output([binary])
            boot_id = '20261121T000000Z-qemu'
            boot = run / 'boots' / boot_id
            boot.mkdir(parents=True)
            (run / 'current').write_text(boot_id + '\n')
            (boot / 'serial.txt').write_bytes(serial)
            (boot / 'ended').touch()
            events = [dict(event='requested-shutdown', action='poweroff'),
                      dict(event='SHUTDOWN', data={'reason': 'guest-shutdown'})]
            (boot / 'events.jsonl').write_text(''.join(json.dumps(event) + '\n' for event in events))
            for image in (*ARTIFACTS['EXT2_DISKS'], *ARTIFACTS['MIGRATION_DISKS']):
                (run / image).write_bytes(image.encode())
            slots = ARTIFACTS['slots_module']()
            slots.write_json(run / 'slots.json', dict(current=None, previous=None, next=None))
            (run / 'build.json').write_text('{}')
            ext2 = ARTIFACTS['disk_digests'](run, ARTIFACTS['EXT2_DISKS'])
            slots.write_json(run / ARTIFACTS['CYCLE_RECORD'], dict(phase='cutover', boots={}, ext2=ext2))
            try:
                ARTIFACTS['finish_cycle'](run, 'cutover', 0)
            except ValueError as error:
                self.fail(f'{error}\n{serial.decode()}')
            self.assertEqual(ARTIFACTS['cycle_state'](run)['boots']['cutover'], boot_id)


if __name__ == '__main__':
    unittest.main()
