"""Drive production file requests, event delivery and presentation over blocked I/O.

Only window/GPU transport and file read/write boundaries are host recorders.
GUSKIT_IO_BASELINE selects the production source revision under test.
"""
import os
import subprocess
import unittest
from guskit_new_save_test import ROOT, run_host
from gt710_gr_live_unwind_test import item


class AsyncFiles(unittest.TestCase):
    def test_save_and_open_acknowledge_a_busy_frame_before_io(self):
        revision = os.environ.get('GUSKIT_IO_BASELINE')
        def source(path):
            return subprocess.check_output(['git', 'show', f'{revision}:{path}'], cwd=ROOT, text=True) if revision else (ROOT / path).read_text()
        app = source('userspace/programs/src/guskit.rs')
        window = source('libs/breengel/src/window.rs')
        names = ['save', 'save_to', 'open_file', 'file_busy', 'request_file', 'start_file_io', 'poll_file_io', 'apply_file_result']
        methods = '\n'.join(item(app, f'    fn {name}(') for name in names if f'    fn {name}(' in app)
        io_module = source('userspace/programs/src/guskit/file_io.rs') if 'mod file_io;' in app else ''
        # These are the I/O boundary replacements, including baseline synchronous I/O.
        methods = methods.replace('std::fs::read(&path)', 'blocked_read(&path)')
        io_module = io_module.replace('std::fs::read(&path)', 'crate::check::blocked_read(&path)')
        frame = item(app[app.index('fn main()'):], '        let presentation_failed = if redraw {') + ' else { false };'
        frame = frame.replace('&mut win', 'win').replace('&mut app', 'app')
        code = (ROOT / 'tests/fixtures/guskit_async_file.rs').read_text()
        code = code.replace('// FILE_IO_MODULE', 'mod file_io { use super::libbreenix;\n' + io_module.replace('//!', '//') + '\n}' if io_module else '')
        code = code.replace('// FILE_IO_FIELD', 'file_io: file_io::State,' if io_module else '')
        code = code.replace('// FILE_IO_INIT', 'file_io: file_io::State::default(),' if io_module else '')
        code = code.replace('// FILE_METHODS', methods)
        code = code.replace('// DELIVER', item(window, '    fn deliver('))
        code = code.replace('// PRESENT_FRAME', item(app, 'fn present_frame(') + '\n' + item(app, 'fn union('))
        code = code.replace('// FRAME', frame)
        code = code.replace('// BUSY_CHECK', 'if app.file_busy() { std::thread::yield_now(); }' if io_module else '')
        code = code.replace('// POLL', 'app.poll_file_io();' if 'fn poll_file_io(' in app else '')
        run_host(self, code.replace('CANVAS_PATH', str(ROOT / 'userspace/programs/src/guskit/canvas.rs'))
                 .replace('FILES_PATH', str(ROOT / 'userspace/programs/src/guskit/files.rs'))
                 .replace('ATOMIC_PATH', str(ROOT / 'libs/libbreenix/src/atomic_file.rs')))


if __name__ == '__main__':
    unittest.main()
