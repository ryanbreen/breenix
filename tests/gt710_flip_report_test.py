"""Same-mode A-B-A needs distinct private bindings and retained full pixels."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import struct
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'))
SWITCH = runpy.run_path(str(ROOT/'tests/gt710_switch_report_test.py'))


def capture(seeds=None):
    serial = SWITCH['switch_capture']()
    memory = API['display_memory'](serial)
    modes = API['native_switches'](serial)
    low, high = modes[0][0], modes[1][0]
    for mode, template in enumerate([low, high]):
        # Low flips precede preferred return; high flips follow it.
        insertion = serial.index('GT710_PREFERRED_RETURN ') if mode == 0 else serial.rindex('GT710_NATIVE_CONSOLE ')
        before = copy.deepcopy(template['active'])
        start = modes[2][0]['put'] if mode == 0 else json.loads(next(l.split(' ',1)[1] for l in serial.splitlines() if l.startswith('GT710_PREFERRED_RETURN ')))['put']
        a, b = [o['start'] for o in memory['objects'][:2]]
        skipped = dict(mode=mode,expected=b,active=before,notifier=0,put=start+16,get=start+16,
                       errors=0,firmware_writes=0)
        addition = 'GT710_FLIP_SKIPPED '+json.dumps(skipped)+'\n'
        previous = start+16
        for step,address in enumerate([a,b,a]):
            e = copy.deepcopy(template)
            e.update(phase='flip',mode=mode,step=step,surface=address,before_active=before,
                     put_before=previous,put=previous+32,get=previous+32,
                     seed=(seeds[mode][step==1] if seeds else
                           0x123450+mode*0x34567+(0x789ab if step==1 else 0)),
                     notifier_before=0,notifier_after=1)
            e['active'] = copy.deepcopy(before)
            next(r for r in e['active'] if r[0]==0x640460)[1]=address>>8
            data=bytearray()
            for y in range(e['timing']['height']):
                first=0xff000000|((e['seed']+y*(e['pitch']//4))&0xffffff)
                addition+=f"GT710_FLIP_PIX {mode} {step} {y} {e['pitch']//4:x}:{first:08x}\n"
                data+=b''.join(struct.pack('<I',0xff000000|((first+x)&0xffffff)) for x in range(e['pitch']//4))
            e.update(bytes=len(data),sha256=hashlib.sha256(data).hexdigest())
            addition+='GT710_FLIP '+json.dumps(e)+'\n'
            before=e['active'];previous=e['put']
        serial=serial[:insertion]+addition+serial[insertion:]
    return serial


class Flips(unittest.TestCase):
    def test_g22_requires_exactly_32_bytes_and_fresh_completion(self):
        serial = capture()
        self.assertEqual(len(API['native_flips'](serial)), 6)
        line = next(l for l in serial.splitlines(keepends=True) if l.startswith('GT710_FLIP '))
        original = json.loads(line.split(' ', 1)[1])
        for size in [0, 8, 16, 24, 28, 36, 40]:
            bad = dict(original, put=original['put_before']+size, get=original['put_before']+size)
            with self.subTest(size=size), self.assertRaises(ValueError):
                API['native_flips'](serial.replace(line, 'GT710_FLIP '+json.dumps(bad)+'\n'))
        for field, value in [('notifier_before', 1), ('notifier_after', 0),
                             ('get', original['put']-4)]:
            bad = dict(original, **{field: value})
            with self.subTest(field=field), self.assertRaises(ValueError):
                API['native_flips'](serial.replace(line, 'GT710_FLIP '+json.dumps(bad)+'\n'))

    def test_g22_production_flip_emits_offset_enable_update_disable_in_order(self):
        # The retained E19 record has no method dump. Execute its production
        # flip + submit functions and check all words, separately from the
        # scorer's observed PUT/GET and fresh-notifier checks.
        import subprocess
        import sys
        import tempfile
        sys.path.insert(0, str(ROOT/'tests'))
        from gt710_gr_live_unwind_test import item
        source = (ROOT/'kernel/src/gpu/gk208/native.rs').read_text()
        code = '\n'.join(item(source, marker) for marker in
                         ('\nfn submit(', '\nfn flip(', 'pub struct FlipSnapshot'))
        fixture = r'''
struct Registers { words: Vec<u32>, put: u32, head: u32, address: u32, raster: u32 }
impl Registers {
    fn read(&self, reg: usize) -> u32 {
        match reg {
            0x640004 => self.put,
            r if r == 0x640460 + self.head as usize * 0x300 => self.address,
            r if r == 0x616340 + self.head as usize * 0x800 => self.raster << 16,
            0x61009c => 0,
            r if r == 0x616308 + self.head as usize * 0x800 => 1,
            r if r == 0x61c004 + self.head as usize * 0x800 => 1,
            r if r == 0x61c030 + self.head as usize * 0x800 => 0,
            _ => panic!("unexpected register"),
        }
    }
    fn delay_us(&mut self, _: u32) { self.raster += 1; }
}
fn write_words(r: &mut Registers, _: u64, address: u64, words: &[u32]) -> Result<(), &'static str> {
    if address >= 4096 { r.words.extend_from_slice(words); }
    Ok(())
}
fn read_word(_: &mut Registers, _: u64, _: u64) -> Result<u32, &'static str> { Ok(0) }
mod modeset {
    pub struct PllLimits; pub struct Pll; pub struct Op;
    pub fn publish(r: &mut super::Registers, put: u32) -> Result<(), &'static str> { r.put=put; Ok(()) }
    pub fn wait(r: &mut super::Registers, reg: usize, _: u32, value: u32) -> Result<(), &'static str> {
        assert_eq!(r.read(reg), value); Ok(())
    }
}
fn complete(_: &mut Registers, _: u64, _: u64, _: &modeset::PllLimits,
            _: &modeset::Pll, _: (u32,u32,bool,bool), _: &[Vec<modeset::Op>;4])
            -> Result<u32, &'static str> { Ok(1) }
CODE
#[test] fn exact_flip_packet() {
    for head in 0..2 {
        let address=0x8000000u64; let mut put=1376;
        let mut r=Registers {words:vec![], put, head, address:(address>>8) as u32, raster:1};
        let snapshot=flip(&mut r, 1<<30, 4096, &mut put, 0, head, head,
            &modeset::PllLimits, &modeset::Pll, &std::array::from_fn(|_| vec![]), address).unwrap();
        assert_eq!(r.words, [0x40460+head*0x300, (address>>8) as u32,
            0x40084, 0x80000000, 0x40080, 0, 0x40084, 0]);
        assert_eq!((put,snapshot.put,snapshot.get), (1408,1408,1408));
        assert_eq!((snapshot.notifier_before,snapshot.notifier_after), (0,1));
    }
}
'''.replace('CODE', code)
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            (p/'flip.rs').write_text(fixture)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority+['rustc', '--edition=2021', '--test', '-Dwarnings',
                                    str(p/'flip.rs'), '-o', str(p/'flip')], check=True)
            subprocess.run([str(p/'flip')], check=True)

    def test_flips_have_a_measured_effort_and_suite_case(self):
        manifest=json.loads((ROOT/'docs/suites/gt710.json').read_text())
        self.assertEqual(next(c for c in manifest['categories'] if c['id']=='flips')['cases'][0]['id'],'aba-handoff')
        API['validate_manifest'](manifest)
        effort=json.loads((ROOT/'docs/efforts/metal.json').read_text())
        row=next(e for e in effort['efforts'] if e['id']=='metal-gt710')
        self.assertEqual(next(m for m in row['milestones'] if m['id']=='flips')['measure'],
                         dict(suite='gt710',categories=['flips']))
        import sys
        sys.path.insert(0,str(ROOT/'scripts/metal'))
        import metal_score
        self.assertEqual(metal_score.host_milestone('gt710-flips.json'),'metal-gt710/flips')

    def test_reused_native_switch_and_other_mode_nonces_are_rejected(self):
        serial=SWITCH['switch_capture']()
        prior=[json.loads(line.split(' ',1)[1])['seed'] for line in serial.splitlines()
               if line.startswith(('GT710_MODE ', 'GT710_SWITCH '))]
        for mode in range(2):
            for buffer in range(2):
                for nonce in prior+[0x123450+(0 if mode else 0x34567)]:
                    seeds=[[0x123450,0x19bdfb],[0x1579b7,0x1d0362]]
                    seeds[mode][buffer]=nonce
                    with self.subTest(mode=mode,buffer=buffer,nonce=nonce):
                        with self.assertRaisesRegex(ValueError,'flip pattern nonce reused'):
                            API['native_flips'](capture(seeds))

    def test_each_edid_mode_requires_latched_aba_full_pixels_and_clean_next_update(self):
        serial=capture()
        frames=API['native_flips'](serial)
        self.assertEqual(len(frames),6)
        for offset in [0,3]:
            self.assertEqual(frames[offset][1],frames[offset+2][1])
            self.assertNotEqual(frames[offset][1],frames[offset+1][1])
        lines=serial.splitlines(keepends=True)
        index=next(i for i,l in enumerate(lines) if l.startswith('GT710_FLIP ') and json.loads(l[11:])['step']==1)
        mutations=[lambda e:e.update(surface=frames[0][0]['surface']),
                   lambda e:e.update(step=0),lambda e:e.update(mode=1),
                   lambda e:e.update(notifier_before=1),lambda e:e.update(notifier_after=0),
                   lambda e:e.update(get=e['put']-4),lambda e:e.update(put_before=0),
                   lambda e:e['active'][5].__setitem__(1,0),
                   lambda e:e.update(before_active=[]),lambda e:e.update(timing=frames[3][0]['timing']),
                   lambda e:e.update(errors=1),lambda e:e.update(underflow=17),
                   lambda e:e.update(power=0),lambda e:e.update(power_status=0x10000000),
                   lambda e:e.update(raster_after=e['raster_before']),
                   lambda e:e.update(firmware_writes=1),lambda e:e.update(bytes=4),
                   lambda e:e.update(sha256='0'*64)]
        for change in mutations:
            broken=lines.copy();e=json.loads(broken[index][11:]);change(e)
            broken[index]='GT710_FLIP '+json.dumps(e)+'\n'
            with self.subTest(change=change),self.assertRaises(ValueError):API['native_flips'](''.join(broken))
        skipped=next(l for l in lines if l.startswith('GT710_FLIP_SKIPPED '))
        for field,value in [('notifier',1),('expected',0),('get',0),('errors',1),('active',[])]:
            e=json.loads(skipped.split(' ',1)[1]);e[field]=value
            with self.subTest(field=field),self.assertRaises(ValueError):
                API['native_flips'](serial.replace(skipped,'GT710_FLIP_SKIPPED '+json.dumps(e)+'\n'))
        pixel=next(l for l in lines if l.startswith('GT710_FLIP_PIX '))
        bad_pixel=pixel.replace(pixel.split(':')[-1][:8],'ff000000')
        for broken in [serial.replace(pixel,''),serial.replace(pixel,bad_pixel),
                       serial.replace(lines[index],''),serial+lines[index],
                       serial.replace(skipped,''),serial+'GT710_FLIP_REFUSAL runtime=timeout\n',
                       serial+'GT710_NATIVE_CONSOLE surface=0 firmware_writes=1\n']:
            with self.assertRaises(ValueError):API['native_flips'](broken)


if __name__=='__main__':unittest.main()
