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
if __name__=='__main__':unittest.main()
