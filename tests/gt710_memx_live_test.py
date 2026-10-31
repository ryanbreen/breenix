"""C5d checks the kernel calculator against the retained independent C5b model."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('offline', ROOT/'scripts/metal/gt710_memx.py')
offline = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = offline
SPEC.loader.exec_module(offline)

class Live(unittest.TestCase):
    def test_live_inputs_emit_intermediate_only_and_preserve_source_wire_protocol(self):
        fixture = json.loads((ROOT/'tests/fixtures/gt710_c5a_card.json').read_text())
        fixture['registers'] = json.loads((ROOT/'docs/gui/gt710-c5b-registers.json').read_text())
        # A changed inherited timing and MR word must change the live calculation.
        fixtures = [fixture, json.loads(json.dumps(fixture))]
        fixtures[1]['registers']['0x10f304'] = '0x80100002'
        fixtures[1]['registers']['0x10f290'] = '0x12345678'
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            rom = bytearray(256*1024)
            for start, data in fixture['segments'].items():
                data = bytes.fromhex(data); start = int(start, 0)
                rom[start:start+len(data)] = data
            (tmp/'rom.bin').write_bytes(rom)
            checks = []
            for i, f in enumerate(fixtures):
                phase = offline.generate(f)['phases'][0]
                regs = [(int(a,0),int(v,0) if isinstance(v,str) else v) for a,v in f['registers'].items()]
                checks.append('''{
let regs = std::collections::BTreeMap::from(%s);
let mut reads = Vec::new();
let plan = memx::calculate(rom, 0x80406c86, 405000, &mut |a| { reads.push(a); *regs.get(&a).expect("unrecorded read") }).unwrap();
assert_eq!(plan.words(), vec!%s);
assert_eq!(plan.coefficients, [1,29,2,0x1000]);
assert!(reads.contains(&0x10f304));
assert_eq!(plan.cpu_before, vec!%s);
assert_eq!(plan.cpu_after, vec!%s);
}''' % (str(regs),str(phase['words']),str([tuple(o['args']) for o in phase['cpu_before']]),str([tuple(o['args']) for o in phase['cpu_after']])))
            source = '''extern crate alloc;
#[allow(dead_code)] #[path="%s"] mod edid;
#[allow(dead_code)] #[path="%s"] mod modeset;
#[path="%s"] mod memx;
fn main() {
let rom = include_bytes!("rom.bin");
%s
assert!(memx::calculate(rom,0x80406c86,900000,&mut |_|panic!("out of envelope read")).is_err());
protocol_test();
}
''' % (ROOT/'kernel/src/gpu/gk208/edid.rs',ROOT/'kernel/src/gpu/gk208/modeset.rs',ROOT/'kernel/src/gpu/gk208/memx.rs','\n'.join(checks))
            source += (ROOT/'tests/fixtures/gt710_memx_transport.rs').read_text()
            (tmp/'check.rs').write_text(source)
            priority = ['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','--cfg','memx_host_check','-Dwarnings',str(tmp/'check.rs'),'-o',str(tmp/'check')],check=True)
            subprocess.run([str(tmp/'check')],check=True)

if __name__ == '__main__': unittest.main()
