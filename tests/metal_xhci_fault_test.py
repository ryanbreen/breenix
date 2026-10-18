#!/usr/bin/env python3
"""Recovery boot classification, staging and scoring; no hardware needed."""
from metal_tools_test import StateTest, boot, m
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import metal_score


class FaultTest(StateTest):
    def args(self, **changes):
        values = dict(id='20261018T000001Z-metal', mode='console', payload='image',
                      target='branch:fixture', task='', suite=None, inject='xhci-short-read',
                      disk=None, input='sol', session=False, attended=False, wait=30, nmi=False,
                      commit='a' * 40)
        return SimpleNamespace(**dict(values, **changes))

    def test_fault_boot_is_ineligible_and_uses_its_own_terminal_stage(self):
        for inject, name in [('xhci-reset-device', 'reset-device'), ('xhci-short-read', 'short-read')]:
            args = self.args(inject=inject, id='20261018T00000' + str(1 if name == 'reset-device' else 2) + 'Z-metal')
            with patch.object(m, 'load_site', return_value={}):
                d = boot.Driver(args)
            d.create_record()
            record = m.read_json(d.dir / 'run.json')
            self.assertEqual(record['kind'], 'fault-test')
            self.assertFalse(record['evidenceEligible'])
            with self.assertRaisesRegex(ValueError, 'fault tests'):
                metal_score.score(d.dir)
            d.load_definition()
            self.assertEqual(d.required_stages, [[('[xhci-fault] ' + name + ' PASS:', 'contains')]])
            self.assertTrue(d.console_done)
            with patch.object(d, 'console_send', side_effect=AssertionError('must not type')):
                d.challenge_console('Breenix login: ')
            d.t0 = 1
            with self.assertRaises(boot.Finish) as result:
                d.on_line('[xhci-fault] ' + name + ' PASS: input-report-decoded-and-requeued')
            self.assertEqual(result.exception.status, m.EXIT_DONE)
            self.assertFalse(any(d.required_stages))

    def test_rejects_incompatible_fault_options(self):
        for changes in [dict(mode='probe'), dict(session=True), dict(attended=True),
                        dict(disk='root'), dict(payload='none'), dict(input=None)]:
            with self.assertRaises(ValueError):
                boot.validate_xhci_fault(self.args(**changes))
        boot.validate_xhci_fault(self.args())

    def test_scorer_refuses_a_fault_even_with_old_misclassified_record(self):
        directory = m.STATE / 'legacy'
        directory.mkdir()
        m.write_json(directory / 'run.json', {'kind': 'boot', 'inject': 'xhci-short-read'})
        with self.assertRaisesRegex(ValueError, 'fault tests'):
            metal_score.score(directory)



if __name__ == '__main__':
    unittest.main()
