"""The boot manager and persistent install's boot-loop side (docs/metal/README.md, "Boot manager and persistent
install"); no controller is contacted."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing  # noqa: E402

m = existing.m
http = existing.load_script('metal-http')


class LabAnswerTest(existing.StateTest):
    def lease(self, **fields):
        m.write_json(m.STATE / m.LEASE, dict(id='20261019T000000Z-metal', heartbeat=m.now(), **fields))

    def test_the_lease_decides_the_answer(self):
        self.lease(state='powered', kind='boot', mode='suite')
        self.assertEqual(http.manager_answer(), 'test 20261019T000000Z-metal')
        self.lease(state='powered', kind='console-session', mode='console')
        self.assertEqual(http.manager_answer(), 'test 20261019T000000Z-metal')
        self.lease(state='powered', kind='persistent', entry='previous')
        self.assertEqual(http.manager_answer(), 'persistent 20261019T000000Z-metal previous')
        self.lease(state='powered', kind='persistent', entry='current', slotAction='commit')
        self.assertEqual(http.manager_answer(), 'persistent 20261019T000000Z-metal current commit')
        for state in ('claimed', 'quarantined'):
            self.lease(state=state, kind='boot')
            self.assertEqual(http.manager_answer(), 'busy 20261019T000000Z-metal')


if __name__ == '__main__':
    unittest.main()
