"""D4: pin the PID-1 bodies and inventory the admitted GUI role changes."""
import pathlib
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
PATH = 'libs/libbreenix/src/suite.rs'
BASE = 'a0ee971de81168391d0a45f3a21dfd215f72253c'  # result transport, before roles
MAIN = '242f37ad'  # D4's main basis


def historical(commit, path):
    return subprocess.check_output(['git', 'show', f'{commit}:{path}'], cwd=ROOT, text=True)


def item(source, signature):
    start = source.index(signature)
    brace = source.index('{', start)
    depth = 0
    for i in range(brace, len(source)):
        depth += (source[i] == '{') - (source[i] == '}')
        if depth == 0:
            return source[start:i + 1]
    raise AssertionError(signature)


class RunnerRoles(unittest.TestCase):
    def test_pid_one_bodies_equal_main(self):
        source = (ROOT / PATH).read_text()
        main = historical(MAIN, PATH)
        for signature in ['fn sweep()', 'fn idle(']:
            self.assertEqual(item(source, signature), item(main, signature))
        seq = 'libs/libbreenix/src/suite_sequence.rs'
        self.assertEqual((ROOT / seq).read_text(), historical(MAIN, seq))

    def test_only_admitted_role_branches_change_the_runner(self):
        source = (ROOT / PATH).read_text()
        base = historical(BASE, PATH)
        expected = item(base, 'pub fn run_with_finalizer(').replace(
            'let mut result_table_error = None;',
            'let scoped = scoped_gui(self.gui);\n        let mut result_table_error = None;\n        if scoped { result_table_error = start_scoped_runner().err(); }')
        expected = expected.replace(
            'if self.defer_display { screen = Screen::open(false, self.id == "gpu"); }',
            'if scoped { process::exit(1); }\n            if self.defer_display { screen = Screen::open(false, self.id == "gpu"); }', 1)
        expected = expected.replace('finalize();\n        feed.finish(',
            'finalize();\n        if scoped { process::exit(if counts.failed == 0 && reclaimed { 0 } else { 1 }); }\n        feed.finish(')
        self.assertEqual(item(source, 'pub fn run_with_finalizer('), expected)
        expected = item(base, 'fn run_case(').replace('match sweep()', 'match sweep_for_role(gui)')
        expected = expected.replace('keep_record_output(suite, name);',
                                    'keep_record_output(suite, name);\n            detach_transcript();')
        self.assertEqual(item(source, 'fn run_case('), expected)
        for signature in ['pub(super) fn emit(', 'fn keep_record_output(']:
            self.assertEqual(item(source, signature), item(base, signature).replace('Fd::STDOUT', 'transcript_output()'))
        self.assertEqual(item(source, 'fn scoped_gui('), '''fn scoped_gui(gui: bool) -> bool {
    gui && process::getpid().is_ok_and(|pid| pid.raw() != 1)
}''')
        self.assertEqual(item(source, 'fn sweep_for_role('), '''fn sweep_for_role(gui: bool) -> Option<&'static str> {
    if scoped_gui(gui) { descendants::sweep_descendants() } else { sweep() }
}''')
        # New role queries live at this gate only, beyond the two pre-existing PID-1 queries.
        self.assertEqual(source.count('process::getpid()'), base.count('process::getpid()') + 1)
        start = item(source, 'fn start_scoped_runner(')
        self.assertIn('process::set_child_subreaper(true)', start)
        self.assertIn('BREENIX_SUITE_TRANSCRIPT_FD', start)
        self.assertIn('FD_CLOEXEC', start)
        self.assertEqual(source.count('start_scoped_runner()'), 2)
        self.assertEqual(source.count('scoped_gui('), 3)
        self.assertEqual(source.count('descendants::sweep_descendants()'), 1)
        # Mandatory result transport remains table-gated, including deferred displays.
        self.assertIn('if self.gui {\n            for category in self.categories', source)
        self.assertIn('let mut results = if gui {', source)
        self.assertEqual(source.count('ResultPipe::new('), 1)


if __name__ == '__main__':
    unittest.main()
