"""Compile C5c's shared interpreter and sequence checks on the host."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
class RamInit(unittest.TestCase):
    def test_interpreter_and_sequence(self):
        with tempfile.TemporaryDirectory() as tmp:
            command=(['taskpolicy','-b'] if sys.platform=='darwin' else [])+['rustc','--edition=2021','-Dwarnings','--test',str(ROOT/'tests/gt710_ram_init_test.rs'),'-o',tmp+'/ram-init']
            subprocess.run(command,check=True)
            subprocess.run([tmp+'/ram-init'],check=True)
if __name__=='__main__':unittest.main()
