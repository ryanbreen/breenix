"""Progress deadlines with a fake clock and synthetic serial streams."""
import json
from pathlib import Path
import runpy
import re
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
Progress = runpy.run_path(str(ROOT / 'scripts/boot_progress.py'))['BootProgress']


def has_case_override(source):
    import re
    source = re.sub(r"//[^\n]*|/\*.*?\*/", "", source, flags=re.S)
    return bool(re.search(r"\bcase_limit_ms\s*(?:[:=(])", source))


class ProgressTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.serial = Path(self.tmp.name) / 'serial.log'
        self.progress = Progress(self.serial, now=0, epoch=0)

    def tearDown(self):
        self.tmp.cleanup()

    def feed(self, text, at, source='user', userspace=False):
        self.progress.observe(text, source, now=at, epoch=at, userspace=userspace)

    def test_fail_then_census_stalls_and_records_last_line(self):
        self.feed('SIGKILL_TEARDOWN_TEST: FAIL\n', 4)
        for at in range(5, 125):
            self.feed(f'[DISPATCH_STRAND_CENSUS:seq={at}:tick={at}]\n', at, str(at))
            self.assertEqual(self.progress.stalled(now=at), at >= 124)
        result = self.progress.record(now=124, epoch=124)
        self.assertEqual(result['status'], 'stalled')
        self.assertEqual(result['last_progress_line'], 'SIGKILL_TEARDOWN_TEST: FAIL')
        self.assertEqual(result['stopped_at'], '1970-01-01T00:02:04+00:00')
        self.assertEqual(json.loads(Path(str(self.serial)+'.stall.json').read_text()), result)

    def test_slow_but_progressing_boot_outlives_hard_default(self):
        text = ''
        for case in range(40):
            at = case * 110
            text += f'SUITE memory CASE pages/case-{case} PASS ms=0\n'
            self.feed(text, at)
            self.assertFalse(self.progress.stalled(now=at + 119))
        self.assertTrue(self.progress.stalled(now=at + 120))

    def test_partial_bytes_never_reset_progress(self):
        self.feed('SIGKILL_TEARDOWN_TEST: FAIL\n', 1)
        for at in range(2, 122):
            self.feed('SIGKILL_TEARDOWN_TEST: FAIL\nPROBE unfinished' + '.' * at, at)
        self.assertTrue(self.progress.stalled(now=121))

    def test_stage_counts_once_and_announcements_never_count(self):
        stage = 'Kernel initialization complete'
        self.feed(f'[ INFO] kernel: {stage}\n', 10, 'kernel')
        self.feed(f'[ INFO] kernel: {stage}\n', 129, 'repeat')
        self.feed('kernel::test_exec: -> Userspace will emit FILE_READ_TEST_PASSED marker if successful\n', 129, 'loader')
        self.assertTrue(self.progress.stalled(now=130))
        self.assertEqual(self.progress.last_line, f'[ INFO] kernel: {stage}')

    def test_stage_alternatives_and_longer_names(self):
        self.assertIn('Kernel initialization complete!', self.progress.markers)
        self.feed('kernel::test_exec: -> Userspace will emit EXEC_ARGV_TEST_PASSED marker\n', 1, 'announcement')
        self.assertFalse(self.progress.stages)
        self.feed('EXEC_ARGV_TEST_PASSED\n', 2, 'user')
        self.assertNotIn('ARGV_TEST_PASSED', self.progress.stages)
        self.feed('[ INFO] kernel: Kernel initialization complete!\n', 3, 'kernel')
        self.assertIn('Kernel initialization complete!', self.progress.stages)

    def test_userspace_suite_probe_stage_and_plain_output_count(self):
        for line in ('SUITE threads CASE lifecycle/create-join PASS ms=1',
                     'PROBE memory OK', 'FILE_READ_TEST_PASSED',
                     '[init] Starting boot script'):
            self.feed(line + '\n', 110, line, userspace=True)
            self.assertFalse(self.progress.stalled(now=229))

    def test_kernel_diagnostics_do_not_count_on_either_serial(self):
        for source in ('mixed', 'user', 'kernel'):
            self.feed('[ INFO] kernel::task: thread count=100\n[CPU_CENSUS:seq=23]\n'
                      'T0[RESUME_PC_CENSUS:cpu=0:seq=24]\n'
                      '[smp] user-thread dispatches per CPU: cpu0=4\n'
                      '[diagnostic] still running\n[clock] UTC epoch=123 source=UEFI\n', 119, source, userspace=source=='user')
        self.assertTrue(self.progress.stalled(now=120))

    def test_census_records_are_not_userspace_output(self):
        self.feed('SIGKILL_TEARDOWN_TEST: FAIL\n', 1, 'serial_user.log', userspace=True)
        self.feed('[TOMBSTONE_CENSUS:resident=0:removed=165]\n[PT_ROOT_CUSTODY:no_proof=0:retired=306]\r\n',
                  121, 'census', userspace=True)
        self.assertTrue(self.progress.stalled(now=121))
        self.feed('[CHILD] x\n', 121, 'child', userspace=True)
        self.assertFalse(self.progress.stalled(now=240))
        self.feed('[PARENT] child reaped\n', 240, 'parent')
        self.assertFalse(self.progress.stalled(now=359))

    def test_plain_kernel_serial_line_is_not_userspace_progress(self):
        self.feed('SIGKILL_TEARDOWN_TEST: FAIL\n', 1, 'serial_user.log', userspace=True)
        self.feed('scheduler still waiting\n', 119, 'serial_kernel.log')
        self.assertTrue(self.progress.stalled(now=121))
        self.assertEqual(self.progress.last_line, 'SIGKILL_TEARDOWN_TEST: FAIL')

    def test_x86_com1_kernel_debug_after_fail_is_not_progress(self):
        self.feed('SIGKILL_TEARDOWN_TEST: FAIL\n', 1, 'serial_user.log', userspace=True)
        for line in ('BP handler: About to call preempt_enable',
                     'BP handler: Called preempt_enable, exiting handler',
                     '[TIMER_SCALE_ORACLE:x86:PASS]', 'breenix> '):
            self.feed(line + '\n', 119, line, userspace=True)
        self.assertTrue(self.progress.stalled(now=121))
        self.assertEqual(self.progress.last_line, 'SIGKILL_TEARDOWN_TEST: FAIL')

    def test_file_poll_keeps_incomplete_line_until_newline(self):
        self.serial.write_bytes(b'SIGKILL_TEARDOWN_TEST: FAIL\nSUITE threads CASE partial')
        self.progress.poll([(self.serial, True)], now=2, epoch=2)
        self.assertEqual(self.progress.last_line, 'SIGKILL_TEARDOWN_TEST: FAIL')
        with self.serial.open('ab') as stream:
            stream.write(b' PASS ms=1\n')
        self.progress.poll([(self.serial, True)], now=3, epoch=3)
        self.assertEqual(self.progress.last_line, 'SUITE threads CASE partial PASS ms=1')

    def test_long_silent_case_requires_explicit_named_allowance(self):
        # Blanket overrides remain forbidden. Only the runner's named
        # allowances may exceed 10 s (the RTC measurement and the breengel
        # workloads plan section 19 names), and each must fit the stall deadline.
        self.feed('SUITE threads START cases=141\n', 0)
        self.assertTrue(self.progress.stalled(now=121))
        # Inspect both builder calls and public-field overrides in the program
        # tree, including helpers imported by suite entry points.
        for source in (ROOT / 'userspace/programs/src').rglob('*.rs'):
            self.assertFalse(has_case_override(source.read_text()), str(source))
        runner = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        self.assertIn('pub const DEFAULT_CASE_LIMIT_MS: u64 = 10_000;', runner)
        table = runner.split('pub const NAMED_CASE_ALLOWANCES:', 1)[1].split(';', 1)[0]
        table = re.sub(r'//[^\n]*|/\*.*?\*/', '', table.split('= &[', 1)[1], flags=re.S)
        entries = re.findall(r'\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,\s*([\d_]+)\s*\)', table)
        self.assertEqual(table.count('('), len(entries), 'every allowance must be parsed')
        allowances = {(suite, case): int(limit.replace('_', '')) for suite, case, limit
                      in entries}
        self.assertEqual(len(entries), len(allowances), 'allowance names must be unique')
        monitor = runpy.run_path(str(ROOT / 'scripts/boot_progress.py'))
        self.assertEqual(allowances, {
            ('time', 'clocks/rate-counter'): 25_000,
            ('breengel', 'font-parser/corpus'): 20_000,
            ('breengel', 'inflate/byte-split'): 20_000,
            ('breengel', 'png-decode/independent-fixtures'): 20_000,
            ('breengel', 'windows/churn'): 20_000,
            ('breengel', 'composition/present-rate'): 20_000,
        })
        for (suite, case), limit in allowances.items():
            manifest = json.loads((ROOT / f'docs/suites/{suite}.json').read_text())
            names = {f"{category['id']}/{item['id']}" for category in manifest['categories']
                     for item in category['cases']}
            self.assertIn(case, names)
            self.assertGreater(limit, 10_000)
            self.assertLess(limit, monitor['STALL_SECONDS'] * 1000)

    def test_unknown_unlabelled_kernel_print_is_not_progress(self):
        self.feed('SUITE smoke START cases=5\n', 0)
        self.feed('new periodic scheduler status\n', 119, 'mixed')
        self.feed('[new_kernel_component] periodic status\n[timer] periodic timer status\n', 119, 'kernel-tags')
        self.feed('new periodic kernel message\n', 119, 'serial_user.log', userspace=True)
        self.assertTrue(self.progress.stalled(now=120))

    def test_userspace_tags_and_last_userspace_survive_kernel_stage(self):
        for tag in ('ntp', 'http_fetch_test', 'net_test', 'bwm', 'bterm'):
            self.feed(f'[{tag}] completed request\n', 10, tag)
            self.assertFalse(self.progress.stalled(now=129))
        self.feed('[ INFO] kernel: Kernel initialization complete!\n', 20, 'kernel')
        result = self.progress.record(now=140, epoch=140)
        self.assertEqual(result['last_userspace_progress_line'], '[bterm] completed request')
        self.assertEqual(result['last_userspace_progress_at'], '1970-01-01T00:00:10+00:00')
        self.assertNotIn('timer', self.progress.user_tags)

    def test_case_override_detection_covers_public_field_and_builder(self):
        for source in ('Suite { case_limit_ms: 121_000, ..suite }',
                       'suite.case_limit_ms = 121000;', 'suite.case_limit_ms (121000)'):
            self.assertTrue(has_case_override(source), source)

    def test_done_only_disarms_when_monitor_explicitly_tracks_completion(self):
        self.feed('PROBE DONE passed=16 failed=0\n', 0)
        self.assertTrue(self.progress.stalled(now=120))
        self.progress.track_completion = True
        self.feed('PROBE DONE passed=16 failed=0\n', 0, 'other')
        # A duplicate does not change state; a genuine first DONE does.
        self.progress = Progress(self.serial, now=0, epoch=0, track_completion=True)
        self.feed('SUITE threads DONE passed=141 failed=0 skipped=0 total=141\n', 0)
        self.assertFalse(self.progress.stalled(now=1000))
        self.feed('SUITE_SEQUENCE READY threads\n', 2)
        self.assertFalse(self.progress.stalled(now=122))
        self.feed('SUITE time START cases=89\n', 123)
        self.assertTrue(self.progress.stalled(now=243))


if __name__ == '__main__':
    unittest.main()
