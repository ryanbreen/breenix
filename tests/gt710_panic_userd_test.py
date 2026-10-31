"""A PRAMIN storage write does not notify the GPU's BAR1 USERD aperture."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]

class UserdPublication(unittest.TestCase):
    def test_terminal_packets_require_bar1_publication(self):
        source = (ROOT/'kernel/src/gpu/gk208/emergency.rs').read_text()
        body = source[source.index('struct Terminal('):source.index('/// Preparation only: map immutable')]
        for start,end in [('    /// Preparation only: share','    pub(crate) fn packet('),
                          ('    pub(crate) fn prepare(','    pub(crate) fn bound(')]:
            a=body.index(start);b=body.index(end,a);body=body[:a]+body[b:]
        fixture=(ROOT/'tests/fixtures/gt710_panic_transport.rs').read_text()
        # Keep the existing receipt, ring-wrap, timeout and display assertions.
        # Only the mock device changes: BAR1 publication, not a VRAM store,
        # advances the device and writes an engine receipt.
        fixture=fixture.replace('if a==0x10008c {','if a==0x10008c && false {')
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'test.rs').write_text(fixture.replace('// TRANSPORT',body))
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','--test','--cfg','feature="capture_panic_oracle"','-Dwarnings',str(p/'test.rs'),'-o',str(p/'test')],check=True)
            subprocess.run([str(p/'test'),'--test-threads=1'],check=True)
if __name__=='__main__':unittest.main()
