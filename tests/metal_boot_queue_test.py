"""FIFO lease admission and detached launches, without controller or power access."""
import contextlib
import io
import json
import os
from types import SimpleNamespace
import threading
import time
import unittest
from unittest.mock import Mock, patch

import metal_tools_test as existing

m, boot = existing.m, existing.boot
run = existing.load_script('metal-run')


class BootQueueTest(existing.StateTest):
    def test_two_launches_second_waits_then_executes_without_refusal(self):
        self.ready()
        entered, release = threading.Event(), threading.Event()
        drivers, threads, outcomes = {}, [], {}

        def start(command, **options):
            boot_id = command[command.index('--id') + 1]
            args = SimpleNamespace(id=boot_id, mode='default', payload='hang', session=False,
                                   attended=False, wait=1, input=None, disk=None, target='main',
                                   task='', commit='', inject=None, nmi=False, no_wait=True)
            with patch.object(m, 'load_site', return_value={}):
                driver = boot.Driver(args)
            drivers[boot_id] = driver
            for method in ('prepare_payload', 'preflight', 'do_stage', 'start_console', 'power_on', 'pump_serial'):
                setattr(driver, method, Mock())

            def watch():
                if boot_id.endswith('01Z-metal'):
                    entered.set()
                    if not release.wait(10):
                        raise RuntimeError('holder was not released')
                raise boot.Finish(0, 'fixture executed')

            def cleanup(status, **kwargs):
                driver.stopping.set()
                m.close_record(driver.dir, status, 'driver')
                if driver.claimed:
                    m.release_lease(boot_id)
            driver.watch, driver.cleanup = watch, cleanup
            thread = threading.Thread(target=lambda: outcomes.update({boot_id: driver.run()}))
            threads.append(thread)
            thread.start()

        first, second = '20261019T000001Z-metal', '20261019T000002Z-metal'
        try:
            with patch.object(boot.subprocess, 'run', side_effect=start), \
                 patch.object(m, 'holder_alive', return_value=True), \
                 patch.object(m, 'unit_active', return_value=True):
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    self.assertEqual(boot.launch(SimpleNamespace(id=first), []), 0)
                    self.assertTrue(entered.wait(5))
                    self.assertEqual(boot.launch(SimpleNamespace(id=second), ['--no-wait']), 0)
                claims = [json.loads(line) for line in output.getvalue().splitlines()]
                self.assertEqual(claims[1]['status'], 'waiting')
                self.assertEqual((claims[1]['holder'], claims[1]['position']), (first, 1))
                lease = m.read_lease()
                self.assertEqual(lease['id'], first)
                self.assertEqual([q['id'] for q in lease['queue']], [second])
                self.assertFalse((drivers[second].dir / 'ended').exists())
                release.set()
                for thread in threads:
                    thread.join(10)
                    self.assertFalse(thread.is_alive())
        finally:
            release.set()
            for thread in threads:
                thread.join(10)
        self.assertEqual(outcomes, {first: 0, second: 0})
        for driver in drivers.values():
            self.assertEqual((driver.dir / 'exit-status').read_text().strip(), '0')
        self.assertEqual(m.read_queue(), [])
        self.assertIsNone(m.read_lease())

    def test_fifo_no_barging_dead_waiters_and_handoff_preserve_position(self):
        self.ready()
        m.claim_lease({'id': 'holder', 'pid': os.getpid()}, wait=False)
        m.write_queue([{'id': 'dead', 'pid': 99999999, 'deadline': m.now() + 60},
                       {'id': 'expired', 'pid': os.getpid(), 'deadline': 0},
                       {'id': 'ahead', 'pid': os.getpid(), 'deadline': m.now() + 60}])
        observed = []

        def advance(state):
            observed.append(state)
            self.assertEqual(state['position'], 2)
            with self.assertRaises(m.Busy):
                m.claim_lease({'id': 'barger', 'pid': os.getpid()}, wait=False)
            self.assertTrue(m.handoff_lease('holder', {'id': 'readback', 'pid': os.getpid()}))
            self.assertEqual([q['id'] for q in m.read_lease()['queue']], ['ahead', 'last'])
            m.release_lease('readback')
            m.claim_lease({'id': 'ahead', 'pid': os.getpid()}, wait=False)
            m.release_lease('ahead')

        m.claim_lease({'id': 'last', 'pid': os.getpid()}, wait=True, poll=0,
                      on_wait=advance)
        self.assertEqual(m.read_lease()['id'], 'last')
        self.assertEqual(len(observed), 1)

    def test_timeout_and_cancel_remove_only_the_waiter_without_touching_power(self):
        self.ready()
        m.claim_lease({'id': 'holder', 'pid': os.getpid()}, wait=False)
        m.update_lease('holder', ceilingAt=500, powerOnAt=100)
        with patch.object(m, 'bmc') as power:
            with self.assertRaisesRegex(m.Busy, 'expired'):
                m.claim_lease({'id': 'timeout', 'pid': os.getpid()}, wait=True,
                              poll=0.001, wait_seconds=0.01)
            def cancel():
                raise RuntimeError('cancelled')
            with self.assertRaisesRegex(RuntimeError, 'cancelled'):
                m.claim_lease({'id': 'cancel', 'pid': os.getpid()}, wait=True, checkpoint=cancel)
            power.assert_not_called()
        self.assertEqual(m.read_queue(), [])
        lease = m.read_lease()
        self.assertEqual((lease['id'], lease['ceilingAt'], lease['powerOnAt']), ('holder', 500, 100))

    def test_unattended_wait_is_forwarded_and_bounded_even_with_no_wait(self):
        args = SimpleNamespace(mode='default', payload='hang', input=None, disk=None,
                               inject=None, sha=None, branch=None, suite=None, no_wait=True,
                               nmi=False, serve_as=None, content_type=None, ramdisk=True, wait=2)
        with patch.object(run, 'host_env', return_value={}), \
             patch.object(run, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='waiting', stderr='')) as launch:
            self.assertEqual(run.boot(args), 0)
        command = launch.call_args.args
        self.assertEqual(command[command.index('--wait') + 1], '2')
        for invalid in (0, -1, float('nan'), float('inf'), 241):
            args.wait = invalid
            with self.assertRaises(SystemExit):
                run.boot(args)


if __name__ == '__main__':
    unittest.main()
