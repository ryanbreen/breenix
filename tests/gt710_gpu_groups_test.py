"""Sample actual method-group boundaries without changing the native packets."""
from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]


class GpuGroups(unittest.TestCase):
    def test_group_export_is_parseable_and_drained(self):
        source=r'''
extern crate alloc;
#[allow(dead_code)] // Export a row through the production timing recorder.
#[path="TIMING"]mod timing;
fn main(){let mut r=timing::Recorder::new();r.gpu_group([1;32]);println!("{}",r.drain(1).unwrap());assert!(r.drain(2).is_none());}
'''.replace('TIMING',str(ROOT/'kernel/src/gpu/gk208/timing.rs'))
        priority=['taskpolicy','-b']if sys.platform=='darwin'else[]
        with tempfile.TemporaryDirectory(prefix='gt710-groups-export-')as directory:
            p=Path(directory);(p/'check.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            text=subprocess.check_output([str(p/'check')],text=True).split(' ',1)[1]
            data=json.loads(text)
            self.assertEqual(len(data['gpu_draw_group_fields']),32)
            self.assertEqual(data['gpu_draw_groups'],[[1]*32])
            self.assertEqual(data['dropped'],0)

    def test_group_queries_extents_state_and_device_results(self):
        source=r'''
extern crate alloc;
#[allow(dead_code)]
#[path="CLOCK"]mod gpu_clock;
#[path="GROUPS"]mod groups;
fn emit(w:&mut Vec<u32>,m:u32,d:&[u32]){w.push(0x20000000|((d.len()as u32)<<16)|m/4);w.extend(d);}
fn main(){
 let mut w=Vec::new();
 for n in 0..6{
  emit(&mut w,0x800,&[0,0x400000,256,256,0xcf,0x30,1,0,0]);
  emit(&mut w,0xa00,&[128f32.to_bits(),128f32.to_bits(),1f32.to_bits(),128f32.to_bits(),128f32.to_bits(),0]);
  emit(&mut w,0xc00,&[256<<16,256<<16,0,1f32.to_bits()]);
  emit(&mut w,0xe00,&[1,96|(160<<16),96|(160<<16)]);
  emit(&mut w,0x1608,&[0,0x800000]);emit(&mut w,0xda4,&[0x1010]);
  emit(&mut w,0x2140,&[0x51,n*0x100]);emit(&mut w,0x1618,&[4]);emit(&mut w,0xd74,&[0,6]);emit(&mut w,0x1614,&[0]);
  if n==5{emit(&mut w,0x110,&[0]);}
 }
 let (plain,none)=groups::instrument(w.clone(),1).unwrap();assert_eq!(plain,w);assert!(none.is_empty());
 let (timed,states)=groups::instrument(w.clone(),256).unwrap();assert_eq!(states.len(),4);
 let mut original=Vec::new();let mut at=0;let mut queries=0;
 while at<timed.len(){let n=((timed[at]>>16)&0x1fff)as usize;let m=(timed[at]&0x1fff)*4;
  if m==0x1b00{assert_eq!(n,4);let address=(u64::from(timed[at+1])<<32)|u64::from(timed[at+2]);
   assert!((0x02000000+2112..0x02000000+4096).contains(&address));assert!([0xf002,0x0980a002].contains(&timed[at+4]));queries+=1;}
  else{original.extend_from_slice(&timed[at..at+n+1]);}at+=n+1;
 }
 assert_eq!(original,w);assert_eq!(queries,28);
 for (i,s)in states.iter().enumerate(){assert_eq!(&s.0[..4],&[0,0x400000,256,256]);assert_eq!(&s.0[10..13],&[1,96|(160<<16),96|(160<<16)]);assert_eq!(&s.0[17..],&[i as u32*0x100,6,6]);}
 let mut spans=Vec::new();for seq in 1..=4{let (base,len)=groups::span(seq).unwrap();assert!(base>=2112&&base+len<=4096);spans.push((base,base+len));}
 spans.sort();for pair in spans.windows(2){assert!(pair[0].1<=pair[1].0);}
 let mut raw=[[256,0,0,0];7];for(i,t)in[100,110,120,150,160].iter().enumerate(){raw[i][2]=*t;}
 raw[5]=[50,0,99,0];raw[6]=[69,0,155,0];
 let row=groups::decode(256,0,&states[0],raw);assert_eq!(row[31],1);assert_eq!(&row[22..27],&[100,110,120,150,160]);assert_eq!(&row[27..31],&[50,69,99,155]);
 raw[2][0]=255;assert_eq!(groups::decode(256,0,&states[0],raw)[31],0);
 assert!(groups::instrument(vec![0x20090000,1],256).is_err());
}
'''.replace('CLOCK',str(ROOT/'kernel/src/gpu/gk208/gpu_time.rs')).replace('GROUPS',str(ROOT/'kernel/src/gpu/gk208/gpu_groups.rs'))
        priority=['taskpolicy','-b']if sys.platform=='darwin'else[]
        with tempfile.TemporaryDirectory(prefix='gt710-gpu-groups-')as directory:
            p=Path(directory);(p/'check.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)


if __name__=='__main__':
    unittest.main()
