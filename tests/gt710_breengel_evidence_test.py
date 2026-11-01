"""E25 u3: retain 401 boundaries and exact pictures through bounded G12 uploads."""
import hashlib
import io
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import gt710_upload as upload
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
PRIORITY = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []


def rust(source, directory, name, args=()):
    path = directory / (name + '.rs')
    path.write_text(source)
    subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', str(path),
                              '-o', str(directory / name)] + list(args), check=True,
                   env=dict(os.environ, CARGO_BUILD_JOBS='4'))
    return subprocess.check_output([str(directory / name)], text=True)


class Evidence(unittest.TestCase):
    def test_401_heap_boundaries_round_trip_and_refuse_over_bound(self):
        source = (ROOT / 'userspace/programs/src/gt710_heap.rs').read_text()
        code = r'''
extern crate self as libbreenix;
pub mod gpu {pub use gpu_abi as abi;}
mod heap {SOURCE
pub fn fixture()->Vec<u8>{
 let _=boundary as fn(usize,usize,&str);assert!(encode().is_err());
 let mut values=[u64::MAX;wire::WORDS];values[0]=1;
 let mut s=Store::new();s.record(0,401,"",Ok(values));
 for n in 1..=401{s.record(n,401,&format!("raster/case-{n}"),Ok(values));}
 let bytes=s.encode().expect("401 cases plus the initial boundary must fit");
 let mut bad=Store::new();bad.record(0,usize::MAX,"",Ok(values));
 assert!(bad.encode().is_err(),"overflow must refuse, never truncate");
 bytes
}}
fn main(){print!("{}",String::from_utf8(heap::fixture()).unwrap());}
'''.replace('SOURCE', source)
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi',
                '--cfg', 'feature="alloc"', '--crate-type', 'rlib',
                str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(p / 'libgpu_abi.rlib')], check=True)
            text = rust(code, p, 'heap', ['--extern', f'gpu_abi={p / "libgpu_abi.rlib"}'])
        rows = [API['expand_case_heap'](json.loads(line.removeprefix('GT710_CASE_HEAP ')))
                for line in text.splitlines()]
        self.assertEqual([r['boundary'] for r in rows], list(range(402)))
        self.assertEqual([r['case'] for r in rows], [''] + [f'raster/case-{n}' for n in range(1, 402)])
        self.assertTrue(all(set(r['values']) == API['CASE_HEAP_FIELDS'] for r in rows))
        self.assertEqual(rows[-1]['values']['heap_free'], 2**64 - 1)
        # Exercise the receiver's former <=170 check as well as the producer.
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            pixels = bytes([0, 0, 0, 255]); digest = hashlib.sha256(pixels).hexdigest()
            payload = b'1:ff000000\n'
            entries = [dict(case=f'raster/case-{n}', picture=n-1, sha256=digest,
                            bytes=len(payload), width=1, height=1, encoding="rle") for n in [1]]
            manifest = ''.join('GT710_BREENGL_PICTURE '+json.dumps(e,separators=(',', ':'))+'\n' for e in entries).encode()
            startup = b'GT710_PIX 0 1:ff003039\n'
            final = text.encode() + manifest
            def anchor(label, data):
                return f'{label} {len(data)} {hashlib.sha256(data).hexdigest()}\n'
            serial = 'GT710_MODE {"schema":1}\n'+anchor('GT710_BULK', startup)
            serial += 'SUITE breengel START cases=401\n'+anchor('GT710_BREENGL_CHUNK 0', payload)
            serial += ''.join(f'SUITE breengel CASE raster/case-{n} PASS ms=1 sha256={digest}\n' for n in range(1, 402))
            serial += 'SUITE breengel DONE passed=401 failed=0 skipped=0 total=401\n'
            serial += 'SUITE breengel MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=64\n'
            serial += anchor('GT710_CASE_HEAP_BULK', text.encode())+anchor('GT710_BREENGL_MANIFEST_BULK', manifest)+anchor('GT710_FINAL_BULK', final)
            serial += f'GT710_BREENGL_TOTAL bytes={len(startup)+len(payload)+len(final)} uploads=3 picture_bytes={len(payload)} pictures=1 unique=1\n'
            serial += 'SUITE_SEQUENCE READY breengel\n'
            for name, data in [('gt710-pixels.rle', startup), ('gt710-final.rle', final), ('gt710-breengel-0000.rle', payload)]:
                (p/name).write_bytes(data)
            (p/'serial.txt').write_text(serial)
            recovered = API['capture_serial'](p)
            self.assertEqual(recovered.count('GT710_CASE_HEAP {'), 402)

    def test_picture_chunks_round_trip_deduplicate_and_reject_damage(self):
        # Use production RLE/manifest generation and the host consumer, with a
        # picture larger than one slot and a duplicate in the next case.
        path = ROOT / 'userspace/programs/src/gt710_artifacts/pictures.rs'
        self.assertTrue(path.exists(), 'native judged-picture sequence producer is missing')
        code = r'''
extern crate self as libbreenix;
pub mod crypto {#[path="SHA"]pub mod sha256;}
#[path="PICTURES"]mod pictures;
fn main(){
 let dir=std::env::var("EVIDENCE_OUT").unwrap();
 let mut store=pictures::Pictures::new();
 let mut chunks=Vec::new();
 let mut send=|data:&[u8]|{chunks.push(data.to_vec());Ok(())};
 let pixels:Vec<u8>=(0..600000u32).flat_map(|v|[v as u8,(v>>8)as u8,(v>>16)as u8,255]).collect();
 for case in ["raster/first","raster/second"] {
  store.record(case, true,1000,600,&pixels,crypto::sha256::sha256(&pixels),&mut send).unwrap();
 }
 let manifest=store.manifest().unwrap();
 assert_eq!(chunks.len(),2);assert!(chunks.iter().all(|b|b.len()<=2<<20));
 for (n,b) in chunks.iter().enumerate(){std::fs::write(format!("{dir}/gt710-breengel-{n:04}.rle"),b).unwrap();}
 std::fs::write(format!("{dir}/manifest"),manifest).unwrap();
 let solid=vec![0,0,0,255];
 store.record("text/first",false,1,1,&solid,crypto::sha256::sha256(&solid),&mut send).unwrap();
 store.record("text/later",false,1,1,&solid,crypto::sha256::sha256(&solid),&mut send).unwrap();
 assert_eq!(store.manifest().unwrap().split(|b|*b==b'\n').filter(|b|!b.is_empty()).count(),3);
 let mut failed=pictures::Pictures::new(); failed.bytes=pictures::MAX_TOTAL-1;
 let mut sent=0;
 let mut sink=|_:&[u8]|{sent+=1;Ok(())};
 let excessive:Vec<u8>=(0..6400000u32).flat_map(|v|[v as u8,(v>>8)as u8,(v>>16)as u8,255]).collect();
 assert!(failed.record("raster/huge",true,3200,2000,&excessive,crypto::sha256::sha256(&excessive),&mut sink).is_err());
 assert_eq!(sent,0,"over-bound pictures must refuse before upload");
 assert!(failed.manifest().is_err(),"a refused picture must never disappear from a successful manifest");
}
'''.replace('SHA', str(ROOT / 'libs/libbreenix/src/crypto/sha256.rs')).replace('PICTURES', str(path))
        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ, EVIDENCE_OUT=tmp):
            p = Path(tmp)
            rust(code, p, 'pictures')
            manifest = (p / 'manifest').read_bytes()
            entries = [json.loads(line.removeprefix('GT710_BREENGL_PICTURE ')) for line in manifest.decode().splitlines()]
            self.assertEqual(len(entries), 2)
            self.assertEqual(entries[0]['sha256'], entries[1]['sha256'])
            self.assertEqual(entries[0]['bytes'], 2400000)
            self.assertEqual(entries[0]['encoding'], 'raw')
            self.assertEqual([e['picture'] for e in entries], [0, 1])
            startup = b'GT710_PIX 0 1:ff003039\n'
            final = manifest
            (p / 'gt710-pixels.rle').write_bytes(startup)
            (p / 'gt710-final.rle').write_bytes(final)
            def anchor(label, data):
                return f'{label} {len(data)} {hashlib.sha256(data).hexdigest()}\n'
            serial = 'GT710_MODE {"schema":1}\n' + anchor('GT710_BULK', startup)
            serial += 'SUITE breengel START cases=2\n'
            for index, entry in enumerate(entries):
                serial += f"SUITE breengel CASE {entry['case']} {'PASS' if index == 0 else 'FAIL'} ms=1 sha256={entry['sha256']}\n"
                if index == 0:
                    for n in range(2):
                        data = (p / f'gt710-breengel-{n:04}.rle').read_bytes()
                        serial += anchor(f'GT710_BREENGL_CHUNK {n}', data)
            serial += 'SUITE breengel DONE passed=1 failed=1 skipped=0 total=2\n'
            serial += 'SUITE breengel MEMORY reclaimed=PASS free_before=100 free_after=100 allowance=64\n'
            serial += anchor('GT710_BREENGL_MANIFEST_BULK', manifest) + anchor('GT710_FINAL_BULK', final)
            serial += f'GT710_BREENGL_TOTAL bytes={len(startup)+len(final)+2400000} uploads=4 picture_bytes=2400000 pictures=2 unique=1\n'
            serial += 'SUITE_SEQUENCE READY breengel\n'
            (p / 'serial.txt').write_text(serial)
            result = API['capture_serial'](p)
            self.assertIn('GT710_BREENGL_PICTURE ', result)
            self.assertNotIn('1:ff000000', serial, 'serial must contain digests, never pixel runs')
            for bad in [serial.replace(entries[1]['sha256'], '0'*64, 1),
                        serial.replace('GT710_BREENGL_CHUNK 1 ', 'GT710_BREENGL_CHUNK 2 '),
                        serial.replace('uploads=4', 'uploads=3'),
                        serial.replace('picture_bytes=2400000', 'picture_bytes=1'),
                        serial + manifest.decode()]:
                (p / 'serial.txt').write_text(bad)
                with self.assertRaises(ValueError): API['capture_serial'](p)
            (p / 'serial.txt').write_text(serial)
            file = p / 'gt710-breengel-0001.rle'
            original = file.read_bytes()
            for data in [original[:-1], bytes([original[0] ^ 1]) + original[1:]]:
                file.write_bytes(data)
                with self.assertRaises(ValueError): API['capture_serial'](p)

    def test_receiver_enforces_sequence_lease_slot_and_per_boot_bounds(self):
        class Handler:
            def __init__(self, route, data, length=None):
                self.path = route; self.rfile = io.BytesIO(data); self.code = None
                self.connection = type('Connection', (), {'settimeout': lambda *_: None})()
                self.headers = {'Content-Length': str(len(data) if length is None else length),
                                'X-Content-SHA256': hashlib.sha256(data).hexdigest()}
            def send_error(self, code, *_): self.code = code
            def send_response(self, code): self.code = code
            def send_header(self, *_): pass
            def end_headers(self): pass
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp); boot = state / 'boots/sample'; boot.mkdir(parents=True)
            route = '/gt710-upload/' + 'a'*64
            with patch.object(upload.m, 'STATE', state), patch.object(upload.m, 'read_lease', return_value=dict(id='sample', state='powered')):
                def reset(**changes):
                    intent = dict(id='sample', route=route, complete=True, suite='breengel', bytes=10,
                                  pictureUploads=0, pictureBytes=0)
                    intent.update(changes)
                    (state / 'gt710-upload.json').write_text(json.dumps(intent))
                reset()
                h = Handler(route+'/pictures/0', b'1:ff000000\n'); upload.receive(h)
                self.assertEqual(h.code, 200, 'BreenGL needs sequential picture uploads after startup')
                self.assertEqual((boot/'gt710-breengel-0000.rle').read_bytes(), b'1:ff000000\n')
                for suffix in ('/pictures/0', '/pictures/2'):
                    h = Handler(route+suffix, b'1:ff000000\n'); upload.receive(h)
                    self.assertEqual(h.code, 404)
                for length, changes in [(2**21+1, {}), (20, {'bytes': 64*2**20-10})]:
                    reset(pictureUploads=1, **changes)
                    h = Handler(route+'/pictures/1', b'x'*20, length); upload.receive(h)
                    self.assertEqual(h.code, 400)
                reset(complete=False)
                h = Handler(route+'/pictures/0', b'1:ff000000\n'); upload.receive(h)
                self.assertEqual(h.code, 404)
                reset()
                with patch.object(upload.m, 'read_lease', return_value=dict(id='other', state='powered')):
                    h = Handler(route+'/pictures/1', b'1:ff000000\n'); upload.receive(h)
                    self.assertEqual(h.code, 404)

    def test_hooks_are_x86_only_and_route_is_staged_for_breengel(self):
        source = (ROOT / 'userspace/programs/src/suite_breengel/main.rs').read_text()
        for module in ('gt710_heap', 'gt710_artifacts'):
            self.assertRegex(source, r'#\[cfg\(target_arch\s*=\s*"x86_64"\)\]\s*#\[path\s*=\s*"../'+module+r'.rs"\]\s*mod '+module)
        self.assertIn('.with_case_observer(', source)
        self.assertIn('.with_judged_observer(', source)
        self.assertIn('.with_finalizer(', source)
        self.assertIn('gt710_artifacts::upload()', source)
        self.assertIn('gt710_heap::encode()', source)
        evidence = source[source.index('fn evidence<'):source.index('fn heap_boundary')]
        self.assertNotIn('panic!', evidence, 'G28 refuses evidence without stopping the suite')
        runner = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        self.assertLess(runner.index('if let Err(error) = result {'), runner.index('observe(&name, frame,'))
        self.assertIn('matches!(outcome, Outcome::Fail', runner[runner.index('observe(&name, frame,'):])
        # The guest-side module imports and callback storage must be compiled out
        # on ARM; target ELF inspection accompanies the requested ARM build.
        runner = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        self.assertRegex(runner, r'#\[cfg\(target_arch\s*=\s*"x86_64"\)\]\s*judged_observer:')
        disk = (ROOT / 'scripts/metal/metal_disk.py').read_text()
        self.assertIn("'breengel'", disk[disk.index('native_suite ='):disk.index('staged =')])


if __name__ == '__main__': unittest.main()
