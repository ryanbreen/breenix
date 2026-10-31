# /*
#  * Copyright 2012 Nouveau Community
#  * Copyright 2012 Red Hat Inc.
#  * Copyright 2013 Red Hat Inc.
#  *
#  * Permission is hereby granted, free of charge, to any person obtaining a
#  * copy of this software and associated documentation files (the "Software"),
#  * to deal in the Software without restriction, including without limitation
#  * the rights to use, copy, modify, merge, publish, distribute, sublicense,
#  * and/or sell copies of the Software, and to permit persons to whom the
#  * Software is furnished to do so, subject to the following conditions:
#  *
#  * The above copyright notice and this permission notice shall be included in
#  * all copies or substantial portions of the Software.
# claim-lint:ok: MIT grant in docs/gui/gt710-c1.md.
#  *
#  * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
#  * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
#  * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
#  * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
#  * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
#  * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
#  * OTHER DEALINGS IN THE SOFTWARE.
#  *
#  * Authors: Martin Peres
#  */
# claim-lint:ok: original MIT grant preserved under G1.
"""C1 check first: synthetic board tables and hardware readback, never ROM fixtures.

Layouts: MIT nouveau v6.8 nvkm/subdev/bios/{perf,cstep,boost,volt,vmap,
therm,timing,gpio}.c; field offsets are cited in docs/gui/gt710-c1.md.
"""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))


def image():
    r = bytearray(4096)
    def put(p, n, v): r[p:p+n] = v.to_bytes(n, 'little')
    r[:3] = b'\x55\xaa\x08'; put(0x18, 2, 0x20)
    r[0x20:0x28] = b'PCIR\xde\x10\x8b\x12'; put(0x30, 2, 8); r[0x35] = 0x80
    r[0x40:0x4c] = bytes([255,184,66,73,84,0,0,1,12,6,1,0])
    r[0x4b] = -sum(r[0x40:0x4c]) & 255
    r[0x4c:0x52] = bytes([80,2,0x38,0,0x60,0])
    for field, ptr in [(0,0x100),(8,0x700),(12,0x400),(16,0x600),(32,0x500),(48,0x300),(52,0x200)]: put(0x60+field,4,ptr)
    # Performance v40: padded domain subrecords, disabled entry.
    r[0x100:0x106] = bytes([0x40,6,12,4,7,3])
    for i,(p,g,m,v) in enumerate([(5,810,405,0),(15,1307,900,1),(255,0,0,0)]):
        off=0x106+i*40; r[off]=p; r[off+2]=v
        for j,f in enumerate([g,600,400,m,200,300,100]): put(off+12+j*4,2,0xc000|f)
    # Cstep v10, pstate -> extra point index; extras have voltage IDs.
    r[0x200:0x206]=bytes([0x10,6,4,1,5,2]); put(0x206,2,15<<5); r[0x209]=1
    for i,f in enumerate([1307,1400]): put(0x20a+i*5,2,f); r[0x20e+i*5]=i
    # Boost v11: one base record and one domain constraint.
    r[0x300:0x306]=bytes([0x11,6,6,6,1,1]); put(0x306,2,15<<5)
    put(0x308,2,1307); put(0x30a,2,1400); r[0x30c:0x30e]=bytes([0,100])
    put(0x30e,2,1307); put(0x310,2,1400)
    # Voltage v50 GPIO ranged, signed step. 900 mV, 925 mV.
    r[0x400:0x404]=bytes([0x50,0x1a,4,0]); r[0x404]=2; r[0x406]=1
    put(0x40a,4,900000); put(0x40e,4,950000); put(0x412,4,900000); put(0x416,2,25000)
    # Vmap v20 constant minimum mode, absent link, nontrivial entry stride.
    r[0x500:0x504]=bytes([0x20,13,34,2])
    for i,v in enumerate([900000,925000]):
        p=0x50d+i*34; r[p:p+2]=bytes([2,255]); put(p+2,4,v); put(p+6,4,v)
    # Thermal table core thresholds followed by an ambient section to ignore.
    rows=[(0,0),(4,95*16+3),(7,90*16+2),(8,80*16+4),(0x32,105*16+5),(0,1),(4,42*16)]
    r[0x600:0x604]=bytes([0x10,4,3,len(rows)])
    for i,(tag,v) in enumerate(rows): r[0x604+i*3]=tag; put(0x605+i*3,2,v)
    # Timing v20 with 11 register words, bitfields, one subentry.
    r[0x700:0x706]=bytes([0x20,6,0x33,4,1,1])
    for i in range(11): put(0x706+i*4,4,0x12340000+i)
    put(0x732,2,0x1234); r[0x734:0x739]=bytes([0xe3,2,0xab,0x78,0x56])
    # DCB v40 / GPIO v41: VID0 on line 3, sense true means log1.
    put(0x36,2,0x800); r[0x800:0x804]=bytes([0x40,0x16,0,8]); put(0x80a,2,0x840)
    r[0x840:0x844]=bytes([0x41,4,1,5]); put(0x844,4,3|(4<<8)); r[0x848]=0x40
    r[-1] = -sum(r) & 255
    return bytes(r)


class C1(unittest.TestCase):
    def api(self):
        import gt710_clock
        return gt710_clock

    def test_offline_tables_and_voltage(self):
        a=self.api(); r=image(); d=a.decode_image(r)
        self.assertEqual([(p['pstate'],p['gpc_khz'],p['memory_khz'],p['voltage_id']) for p in d['performance']['levels']],[(5,810000,405000,0),(15,1307000,900000,1)])
        self.assertEqual(d['cstep']['points'][1]['frequency_khz'],1400000)
        self.assertEqual(d['boost']['levels'][0]['domains'][0]['max_khz'],1400000)
        self.assertEqual(d['voltage']['type'],'gpio')
        self.assertEqual(d['voltage']['entries'],[{'vid':0,'uv':900000},{'vid':1,'uv':925000}])
        self.assertEqual(d['voltage_map']['entries'][1]['min_uv'],925000)
        self.assertEqual(d['thermal']['thresholds']['critical'],{'c':95,'hysteresis':3})
        self.assertEqual(d['thermal']['thresholds']['down_clock']['c'],90)
        self.assertEqual(d['timing']['entries'][0]['registers'],list(range(0x12340000,0x1234000b)))
        self.assertEqual(d['timing']['entries'][0]['fields']['2c_003f'],0x34)
        raw={0xd61c:0x4000}
        v=a.voltage_state(d,raw)
        self.assertEqual(v['current_uv'],925000)
        self.assertFalse(v['levels'][-1]['needs_more_voltage'])
        self.assertTrue(a.voltage_state(d,{0xd61c:0})['levels'][-1]['needs_more_voltage'])
        for off in [0x101,0x201,0x301,0x401,0x501,0x601,0x701]:
            b=bytearray(r); b[off]=1; b[-1]=0; b[-1]=-sum(b)&255
            with self.subTest(offset=off), self.assertRaises(ValueError): a.decode_image(bytes(b))
        with self.assertRaises(ValueError): a.decode_image(r[:-1])
        b=bytearray(r); b[100]^=1
        with self.assertRaises(ValueError): a.decode_image(bytes(b))

    def test_pwm_signed_voltage_and_absent_tables(self):
        a=self.api(); r=bytearray(image())
        def put(p,n,v): r[p:p+n]=v.to_bytes(n,'little')
        r[0x404]=1; put(0x405,4,100000); put(0x416,4,200000)
        r[0x846]=0x81 # function byte in GPIO word, actually offset +1 is 0x845
        r[0x845]=0x81
        r[-1]=0; r[-1]=-sum(r)&255
        d=a.decode_image(bytes(r)); self.assertEqual(d['voltage']['type'],'pwm')
        self.assertEqual(a.voltage_state(d,{0x20340:100,0x20344:50})['current_uv'],1000000)
        self.assertIsNone(a.voltage_state(d,{0x20340:0,0x20344:50})['current_uv'])
        r=bytearray(image()); r[0x60+48:0x60+56]=bytes(8); r[-1]=0; r[-1]=-sum(r)&255
        d=a.decode_image(bytes(r)); self.assertEqual(d['boost']['status'],'absent'); self.assertEqual(d['cstep']['status'],'absent')

    def test_extended_read_tree_and_restore_only_sensor(self):
        source=r'''extern crate alloc;
#[path="CLOCK"] mod clock;
#[path="SENSOR"] mod sensor;
use std::collections::BTreeMap;
#[test] fn tree() {
 let mut r=BTreeMap::from([(0x101000,0x400000),(0x137100,7),(0x137000,1),(0x137004,0x11e01),
 (0x137020,1),(0x137024,0x11401),(0x137040,1),(0x137044,0x10a01),
 (0x1373f4,1),(0x132000,1),(0x132004,0x10000201),(0x132020,1),(0x132024,0x11401),(0x132030,0xf0000000)]);
 let (raw,d)=clock::sample_domains(|a|*r.get(&a).unwrap_or(&0));
 assert!(raw.contains(&(0x137024,0x11401)));
 for (n,f) in [("crystal",27000),("href",100000),("gpc",810000),("rop",540000),("hubk07",270000),("hubk06",27000),("hubk01",27000),("pmu",27000),("vdec",27000),("memory",540000),("mpll",540000)] {assert_eq!(d.iter().find(|v|v.0==n).unwrap().1,Ok(f),"{}",n);}
 r.insert(0x13717c,0x40000103);r.insert(0xe820,1);r.insert(0xe824,0x11e01);
 r.insert(0x1370e0,1);r.insert(0x1370e4,0x11401);r.insert(0x13726c,0x80000200);
 assert_eq!(clock::sample_domains(|a|*r.get(&a).unwrap_or(&0)).1.iter().find(|v|v.0=="hubk06").unwrap().1,Ok(270000));
 let s=clock::sample(|a|*r.get(&a).unwrap_or(&0)); let _=(s.raw,s.gpc_khz,s.memory_khz,s.host_khz,s.temperature_c);
}
struct Io {r:BTreeMap<u32,u32>, writes:Vec<u32>,waits:u32}
impl sensor::Io for Io {
 fn read(&mut self,a:u32)->u32 {*self.r.get(&a).unwrap_or(&0)}
 fn write(&mut self,a:u32,v:u32) {assert!([0x22400,0x21000,0x20008,0x2000c].contains(&a));self.writes.push(a);self.r.insert(a,v);}
 fn stabilize(&mut self) {self.waits+=1;}
}
#[test] fn sensor_restores() {
 let mut i=Io{r:BTreeMap::from([(0x22400,0x40),(0x21000,0x20),(0x212a8,1),(0x20400,49),(0x20008,0x8000),(0x2000c,3)]),writes:vec![],waits:0};
 let original=i.r.clone(); let s=sensor::temperature(&mut i);assert_eq!(s.value,Ok(49));assert!(s.restored); assert_eq!(i.r,original);assert_eq!(i.waits,1);assert_eq!(i.writes.len(),8);
 i.r.insert(0x212a8,0); let original=i.r.clone();let s=sensor::temperature(&mut i);assert_eq!(s.value,Err("sensor-unavailable"));assert!(s.restored);assert_eq!(i.r,original);
}
'''.replace('CLOCK',str(ROOT/'kernel/src/gpu/gk208/clock.rs')).replace('SENSOR',str(ROOT/'kernel/src/gpu/gk208/sensor.rs'))
        with tempfile.TemporaryDirectory() as t:
            p=Path(t); (p/'c.rs').write_text(source)
            subprocess.run((['taskpolicy','-b'] if sys.platform=='darwin' else [])+['rustc','--edition=2021','--test',str(p/'c.rs'),'-o',str(p/'c')],check=True)
            subprocess.run([str(p/'c')],check=True)

    def test_benchmark_work_and_completeness(self):
        a=self.api()
        rows=[dict(kind=k,trial=t,begin_ns=100,end_ns=100+n,elapsed_ns=n,
                   work=w,unit=u,completion=True) for k,w,u,n in
              [('core',16*512*512,'fragments',12345678),('memory',64<<20,'bytes',23456789)] for t in (1,2)]
        self.assertEqual(len(a.benchmarks(rows)),4)
        for change in ['missing','work','time','completion','duplicate']:
            b=copy.deepcopy(rows)
            if change=='missing': b.pop()
            elif change=='duplicate': b[3]=b[2]
            elif change=='work': b[0]['work']-=1
            elif change=='time': b[0]['elapsed_ns']-=1
            else: b[0]['completion']=False
            with self.subTest(change=change),self.assertRaises(ValueError): a.benchmarks(b)

    def test_c1_record_and_bulk_authentication(self):
        a=self.api()
        vals={0x101000:0x400000,0x137100:7,0x1373f4:1,0x132000:1,0x132004:0x10000201,
              0x132020:1,0x132024:0x11401,0x132030:0xf0000000,0x137320:0,0x137330:0}
        for i,n in [(0,30),(1,20),(2,10)]:
            vals.update({0x137000+32*i:1,0x137004+32*i:0x10000+(n<<8)+1,0x137120+4*i:0,0x137140+4*i:0})
        for i in [0,1,2,7,8,12,14]:
            vals[0x137250+4*i]=0
            if i>=7:vals.update({0x137160+4*i:0,0x1371d0+4*i:0})
        raw=list(map(list,vals.items()))
        port=dict(bdf='01:00.0',cap_offset=0x60,flags=2,device_cap=0,link_cap=2+(8<<4),link_control=0,link_status=2+(8<<4),slot_cap=None)
        upstream=dict(port,bdf='00:01.0',flags=0x142,slot_cap=75<<7)
        sensor=[[0x22400,0x800],[0x21000,1],[0x212a8,1],[0x21000,1],[0x22400,0x800],[0x20008,0x80000000],[0x2000c,0],[0x20400,49],[0x20008,0x80000000],[0x2000c,0]]
        row=dict(schema=1,phase='startup',raw=raw,domains=a.domains(raw),temperature=dict(value=49,status='ok'),sensor_restored=True,sensor_raw=sensor,sensor_writes=[],voltage_raw=[[0xd610+4*i,0] for i in range(32)]+[[0x20340,0],[0x20344,0]],pcie=[port,upstream])
        self.assertEqual(a.c1_record(row),row)
        self.assertEqual(a.link(upstream)['slot_power_limit_w'],75)
        b=copy.deepcopy(row);b['domains']['gpc']['value']+=1
        with self.assertRaises(ValueError):a.c1_record(b)
        b=copy.deepcopy(row);b['sensor_writes']=[[0x137000,1]]
        with self.assertRaises(ValueError):a.c1_record(b)
        import runpy
        api=runpy.run_path(str(ROOT/'scripts/metal/gt710-suite'),run_name='c1_transport')
        vbios=dict(bytes=len(image()),sha256=hashlib.sha256(image()).hexdigest(),hex=image().hex())
        bodies=[('C1','GT710_CLOCK_C1 '+json.dumps(row,separators=(',',':'))),('VBIOS','GT710_VBIOS_V1 '+json.dumps(vbios,separators=(',',':')))]
        data=''.join(body+'\n' for _,body in bodies).encode()
        serial=''.join(f'GT710_{name}_BULK bytes={len(body)} sha256={hashlib.sha256(body.encode()).hexdigest()}\n' for name,body in bodies)
        serial+=f'GT710_BULK {len(data)} {hashlib.sha256(data).hexdigest()}\n'
        with tempfile.TemporaryDirectory() as t:
            p=Path(t);(p/'serial.txt').write_text(serial);(p/'gt710-pixels.rle').write_bytes(data)
            full=api['capture_serial'](p);self.assertIn(bodies[0][1],full)
            for mutated in [serial.replace('GT710_C1_BULK ','REMOVED '),serial.replace('GT710_VBIOS_BULK ','REMOVED '),serial+bodies[0][1]+'\n']:
                (p/'serial.txt').write_text(mutated)
                with self.assertRaises(ValueError):api['capture_serial'](p)
            (p/'serial.txt').write_text(serial);(p/'gt710-pixels.rle').write_bytes(data.replace(b'49',b'48',1))
            with self.assertRaises(ValueError):api['capture_serial'](p)

    def test_boot_route_has_no_suite(self):
        import runpy
        run=runpy.run_path(str(ROOT/'scripts/metal/metal-run'),run_name='c1_test')
        self.assertEqual(run['kernel_features']('probe',[],None,'clocks'),['--features','gt710_clock_probe'])
        with self.assertRaises(ValueError):run['kernel_features']('probe',['gpu'],None,'clocks')

if __name__=='__main__': unittest.main()
