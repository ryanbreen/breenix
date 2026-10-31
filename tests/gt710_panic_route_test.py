"""CLI selection and a stopped panic companion's retained lifecycle."""
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

from gt710_panic_report_test import API, ROOT, VALID, producer


class PanicRoute(unittest.TestCase):
    def test_argument_parsers_accept_explicit_feature_and_panic_choice(self):
        run = runpy.run_path(str(ROOT / 'scripts/metal/metal-run'))
        main = run['main']
        build = Mock(return_value=0)
        boot = Mock(return_value=0)
        with patch.dict(main.__globals__, build=build, boot=boot):
            with patch.object(sys, 'argv', ['metal-run', '_build', '20261030T000000Z-metal', 'a'*40,
                                            '--features', 'capture_panic_oracle']):
                self.assertEqual(main(), 0)
            self.assertEqual(build.call_args.args[2], 'capture_panic_oracle')
            with patch.object(sys, 'argv', ['metal-run', 'boot', '--gt710-category', 'panic']):
                self.assertEqual(main(), 0)
            self.assertEqual(boot.call_args.args[0].gt710_category, 'panic')

    def test_stopped_panic_companion_rejects_bad_lifecycle_or_build(self):
        sha = 'a'*40
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            boot = dict(producer(VALID)['panic']['boot'], id='20261030T000000Z-metal',kind='boot',platform='metal',arch='x86_64',
                        commit=sha,toolsCommit=sha,evidenceEligible=True,inject=None)
            boot['build']['commit'] = sha
            events = [dict(event='fatal'),dict(event='power-off',off=True),
                      dict(event='closed',status=1,endedBy='driver')]
            contents = {'run.json':json.dumps(boot), 'events.jsonl':'\n'.join(map(json.dumps,events)),
                        'serial.txt':VALID,'ended':'1','dirty':'0','commit':sha,'exit-status':'1'}
            for name, text in contents.items():
                (path/name).write_text(text)
            API['read_panic_capture'](path,sha)
            for name, text in [('exit-status','124'),('ended',''),('dirty','1'),('commit','b'*40),
                               ('run.json',json.dumps(dict(boot,toolsCommit='b'*40))),
                               ('events.jsonl','\n'.join(map(json.dumps,[dict(event='console-gap')]+events))),
                               ('events.jsonl',json.dumps(events[-1])),
                               ('serial.txt',VALID.replace('shown=1','shown=0'))]:
                with self.subTest(file=name):
                    (path/name).write_text(text)
                    with self.assertRaises(ValueError):
                        API['read_panic_capture'](path,sha)
                    (path/name).write_text(contents[name])


if __name__ == '__main__':
    unittest.main()
