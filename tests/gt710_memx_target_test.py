"""Compare each bounded C5e live pass with the independent MIT-source model."""
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


class Target(unittest.TestCase):
    def test_target_and_return_recalculate_from_post_exec_reads(self):
        fixture = json.loads((ROOT/'tests/fixtures/gt710_c5a_card.json').read_text())
        fixture['registers'] = json.loads((ROOT/'docs/gui/gt710-c5b-registers.json').read_text())
        regs = {int(a, 0): int(v, 0) if isinstance(v, str) else v
                for a, v in fixture['registers'].items()}
        decoded = offline.decode(fixture)
        checks = []
        for former, target in ((405000, 900000), (900000, 405000)):
            transition = offline.generate(dict(fixture, registers={hex(a): v for a, v in regs.items()}), former, target)
            self.assertEqual(len(transition['phases']), 2)
            for index, phase in enumerate(transition['phases']):
                # Autonomous MR/timing changes between EXECs must be read live.
                if former == 405000 and index == 1:
                    regs[0x10f304] ^= 0x40000000
                    regs[0x10f290] ^= 0x20
                script = offline.Script(regs)
                before = offline.prog0(script, decoded, 1000)
                offline.ddr3(script, phase['config'], tuple(phase['coefficients'].values()),
                             phase['mode'], decoded['gpio'], fixture['display_present'])
                ops = list(script.ops)
                after = offline.prog0(script, decoded, phase['frequency_khz'])
                words = offline.pack([(o['op'], o['args']) for o in ops if o['emitted']])
                checks.append('''{
let regs = std::collections::BTreeMap::from(%s);
let mut reads = Vec::new();
let plan = memx::calculate_pass(rom,0x80406c86,%d,%d,%s,&mut |a| {reads.push(a);*regs.get(&a).expect("unrecorded live read")}).unwrap();
assert_eq!(plan.words(),vec!%s);
assert_eq!(plan.cpu_before,vec!%s);
assert_eq!(plan.cpu_after,vec!%s);
assert_eq!(plan.coefficients,%s);
assert!(reads.contains(&0x10f304));
assert!(reads.contains(&0x132024));
}''' % (str(list(regs.items())), former, target, str(index == 0).lower(), str(words),
        str([tuple(o['args']) for o in before]), str([tuple(o['args']) for o in after]),
        str(list(phase['coefficients'].values()))))
                regs = {a: v.number for a, v in script.state.items()}
        with tempfile.TemporaryDirectory() as name:
            tmp = Path(name)
            rom = bytearray(256*1024)
            for start, data in fixture['segments'].items():
                data = bytes.fromhex(data); start = int(start, 0)
                rom[start:start+len(data)] = data
            (tmp/'rom.bin').write_bytes(rom)
            source = '''extern crate alloc;
#[allow(dead_code)] #[path="%s"] mod edid;
#[allow(dead_code)] #[path="%s"] mod modeset;
#[path="%s"] mod memx;
fn main() {
let rom = include_bytes!("rom.bin");
%s
assert!(memx::calculate(rom,0x80406c86,900000,&mut |_|panic!("out of envelope read")).is_err());
assert!(memx::calculate_pass(rom,0x80406c86,405000,1307000,false,&mut |_|panic!("out of envelope read")).is_err());
protocol_test();
}
''' % (ROOT/'kernel/src/gpu/gk208/edid.rs', ROOT/'kernel/src/gpu/gk208/modeset.rs',
       ROOT/'kernel/src/gpu/gk208/memx.rs', '\n'.join(checks))
            source += (ROOT/'tests/fixtures/gt710_memx_transport.rs').read_text()
            (tmp/'check.rs').write_text(source)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','--cfg','memx_host_check','-Dwarnings',str(tmp/'check.rs'),'-o',str(tmp/'check')],check=True)
            subprocess.run([str(tmp/'check')],check=True)


if __name__ == '__main__': unittest.main()
