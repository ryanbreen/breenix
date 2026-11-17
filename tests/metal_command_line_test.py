"""Kernel command-line encoding in the early boot intent, without hardware."""
from pathlib import Path
import sys
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts/metal'))
import metal_persist
class CommandLine(unittest.TestCase):
 def test_encode_option_and_reject_line_injection(self):
  self.assertEqual(metal_persist.intent('default',command_line='gt710.clocks=post'),
                   b'BREENIX-BOOT 2\ntarget default\ncmdline gt710.clocks=post\n')
  for bad in ['x\ntarget persistent','x\x00','x'*256,'é']:
   with self.assertRaises(ValueError):metal_persist.intent('default',command_line=bad)
 def test_breenfs_cli_restricts_and_forwards(self):
  from unittest.mock import patch
  from types import SimpleNamespace
  import metal_tools_test as existing
  run = existing.load_script('metal-run')
  parser = run.argument_parser()
  for options in (['--disk', 'root'], ['--disk', 'root', '--suite', 'files-io'],
                  ['--disk', 'root', '--mode', 'console', '--input', 'sol']):
   args = parser.parse_args(['boot', '--root-fs', 'breenfs'] + options)
   with patch.object(run, 'host_env', return_value={}), patch.object(run, 'resolve', return_value='a'*40), \
        patch.object(run, 'resolve_suites', return_value='files-io'), \
        patch.object(run, 'in_metal', return_value=SimpleNamespace(returncode=1, stdout='', stderr='fixture')) as remote:
    self.assertEqual(run.boot(args), 1)
   command = remote.call_args.args
   self.assertEqual(command[command.index('--root-fs') + 1], 'breenfs')
  for options in ([], ['--disk', 'install'], ['--disk', 'root', '--mode', 'shell'],
                  ['--disk', 'root', '--payload', 'none']):
   args = parser.parse_args(['boot', '--root-fs', 'breenfs'] + options)
   with self.assertRaisesRegex(SystemExit, '^--root-fs breenfs needs --disk root and an image boot in default, suite or console mode$'):
    run.boot(args)
if __name__=='__main__':unittest.main()
