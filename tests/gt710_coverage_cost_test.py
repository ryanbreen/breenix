"""Execute the native submit encoder selected by production with a payload budget."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
PRIORITY = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []


class CoverageCost(unittest.TestCase):
    def test_encoded_stream_cost(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        native = core.split('let cost_translate =', 1)[1]
        name = re.search(r'crate::gk208::encode::(\w+)\(&words, &resolved\)', native)[1]
        source = (ROOT / 'tests/gt710_coverage_cost_test.rs').read_text()
        source = source.replace('RUNTIME_TRANSLATE', name).replace('../kernel/', str(ROOT / 'kernel') + '/')
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'check.rs').write_text(source)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', '--cfg', 'feature="alloc"',
                '--crate-name', 'gpu_abi', '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check'), '--test-threads=1', '--nocapture'], check=True)

    def test_execution_cost_records_only_finished_work(self):
        module = (ROOT / 'kernel/src/gpu/gk208/cost.rs').read_text()
        module = module.replace('#[cfg(target_os = "none")]\n', '')
        source = r'''#![allow(dead_code, unused_imports)] // The harness observes only the new runtime slots.
mod signal {
    pub fn monotonic_nanos()->u64 { crate::NOW.load(std::sync::atomic::Ordering::Relaxed) }
}
static NOW:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(100);
#[path="COST"]pub mod cost;
fn main(){
    assert!(cost::execution(cost::Slot::ScratchCopy).is_none());
    cost::snapshot();
    let failed=cost::execution(cost::Slot::ScratchCopy);drop(failed);
    assert!(cost::snapshot()[40..].iter().all(|&v|v==0));
    for (i,slot) in [cost::Slot::ScratchPrepare,cost::Slot::ScratchCopy,
        cost::Slot::DrawExecute,cost::Slot::NativeSubmit].into_iter().enumerate(){
        let start=100+i as u64*100;
        NOW.store(start,std::sync::atomic::Ordering::Relaxed);
        if matches!(slot,cost::Slot::ScratchCopy|cost::Slot::DrawExecute){
            let pending=cost::execution(slot).unwrap();
            NOW.store(start+25,std::sync::atomic::Ordering::Relaxed);pending.finish();
        }else{
            let phase=cost::phase(slot);
            NOW.store(start+25,std::sync::atomic::Ordering::Relaxed);drop(phase);
        }
        assert_eq!(&cost::snapshot()[40+2*i..42+2*i],&[1,25]);
    }
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'cost.rs').write_text(module)
            (p / 'runtime.rs').write_text(source.replace('COST', str(p / 'cost.rs')))
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', '--crate-name', 'gpu_abi',
                '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', '--extern',
                f'gpu_abi={p / "libgpu_abi.rlib"}', str(p / 'runtime.rs'), '-o', str(p / 'runtime')], check=True)
            subprocess.run([str(p / 'runtime')], check=True)

    def test_compact_producer_round_trip_and_refusals(self):
        import hashlib
        import json
        import runpy
        source = (ROOT / 'userspace/programs/src/gt710_heap.rs').read_text()
        harness = r'''
extern crate self as libbreenix;
pub mod gpu { pub use gpu_abi as abi; }
mod heap { SOURCE
    pub fn fixture()->Vec<u8> {
        let _=boundary as fn(usize,usize,&str);let _=encode();
        let mut store=Store::new();
        let mut values=core::array::from_fn(|i|i as u64*123456789);
        values[0]=1;
        store.record(0,1,"",Ok(values));
        store.record(1,1,"performance/coverage-throughput",Ok(values));
        store.encode().unwrap()
    }
}
fn main(){use std::io::Write;std::io::stdout().write_all(&heap::fixture()).unwrap();}
'''.replace('SOURCE', source)
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'producer.rs').write_text(harness)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', '--crate-name', 'gpu_abi',
                '--cfg', 'feature="alloc"', '--crate-type', 'rlib', str(ROOT / 'libs/gpu-abi/src/lib.rs'),
                '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', '--extern',
                f'gpu_abi={p / "libgpu_abi.rlib"}', str(p / 'producer.rs'), '-o', str(p / 'producer')], check=True)
            body = subprocess.check_output([str(p / 'producer')])
            api = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
            names = re.findall(r'"([^"\n]+)"', (ROOT / 'libs/gpu-abi/src/heap_trace.rs').read_text().split('pub const NAMES:',1)[1].split('];',1)[0])
            startup = b'GT710_PIX 0 1:ff003039\n'
            def score(data, forged=False):
                digest=hashlib.sha256(data).hexdigest()
                serial=('GT710_MODE {"schema":1}\n'
                    +f'GT710_BULK {len(startup)} {hashlib.sha256(startup).hexdigest()}\n'
                    +'SUITE gpu START total=1\nSUITE gpu CASE performance/coverage-throughput PASS ms=1\n'
                    +'SUITE gpu DONE passed=1 failed=0 skipped=0 total=1\n'
                    +'SUITE gpu MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=48\n'
                    +f'GT710_CASE_HEAP_BULK {len(data)} {digest}\n'
                    +f'GT710_FINAL_BULK {len(data)} {digest}\nSUITE_SEQUENCE READY gpu\n')
                (p/'serial.txt').write_text(serial);(p/'gt710-pixels.rle').write_bytes(startup)
                (p/'gt710-final.rle').write_bytes(data+b' ' if forged else data)
                return api['capture_serial'](p)
            normalized=score(body)
            for line in normalized.splitlines():
                if line.startswith('GT710_CASE_HEAP '):
                    row=json.loads(line.split(' ',1)[1]);self.assertEqual(set(row['values']),set(names))
                    self.assertEqual([row['values'][n] for n in names],[1]+[i*123456789 for i in range(1,len(names))])
            rows=[json.loads(line.split(' ',1)[1]) for line in body.decode().splitlines()]
            for bad in [[], rows[0]['values']['costs_v1'][:-1], [True]*48, [1<<64]*48]:
                altered=json.loads(json.dumps(rows));altered[0]['values']['costs_v1']=bad
                data=''.join('GT710_CASE_HEAP '+json.dumps(row,separators=(',',':'))+'\n' for row in altered).encode()
                with self.assertRaises(ValueError):score(data)
            with self.assertRaises(ValueError):score(body,forged=True)

    def test_phase_fields_reach_the_case_scorer(self):
        files = ['kernel/src/gpu/gk208/cost.rs', 'libs/gpu-abi/src/heap_trace.rs', 'scripts/metal/gt710-suite']
        for path in files:
            text = (ROOT / path).read_text()
            for phase in ['scratch_prepare', 'scratch_copy', 'draw_execute', 'native_submit']:
                for suffix in ['_n', '_ns']:
                    self.assertIn('"' + phase + suffix + '"' if path.endswith('.rs') else
                                  "'" + phase + suffix + "'", text, (path, phase, suffix))


if __name__ == '__main__':
    unittest.main()
