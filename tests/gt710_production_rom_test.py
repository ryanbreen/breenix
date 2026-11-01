"""Synthetic runtime performance/voltage and RAM-type checks; no board ROM."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_c1_test import image
ROOT=Path(__file__).resolve().parents[1]

class ProductionRom(unittest.TestCase):
 def test_runtime_targets_vid_and_memory_type(self):
  with tempfile.TemporaryDirectory() as t:
   p=Path(t);r=bytearray(image())
   # Add BIT M v2 with a RAMCFG type table: disabled match then DDR3.
   r[0x4a]=2;r[0x4b]=0;r[0x4b]=-sum(r[0x40:0x4c])&255
   r[0x52:0x58]=bytes([77,2,7,0,0x90,0]);r[0x90:0x97]=bytes([1,0,0,0,9,0,0])
   r[0x900:0x90b]=bytes([0x10,7,2,2,0,0,0,0x1f,0,0x11,1])
   (p/'rom').write_bytes(r)
   (p/'check.rs').write_text('''extern crate alloc;
#[allow(dead_code)] #[path="%s"] mod edid;
#[allow(dead_code)] #[path="%s"] mod modeset;
#[allow(dead_code)] #[path="%s"] pub mod perf;
mod clock {pub use super::perf;}
#[path="%s"] mod production_rom;
fn main() {
 let mut r=include_bytes!("rom").to_vec();
 let target=production_rom::target(&r,4).unwrap();
 assert_eq!((target.gpc_khz,target.memory_khz,target.voltage_id),(1307000,900000,1));
 assert_eq!(production_rom::voltage(&r,1,32,0,&mut |a|{assert_eq!(a,0xd61c);0x4000}).unwrap(),(925000,925000));
 assert_eq!(production_rom::voltage(&r,1,32,0,&mut |_|0).unwrap(),(900000,925000));
 r[0x909]=0x15;let refusal=production_rom::target(&r,4).unwrap_err();
 assert_eq!(refusal.check,"ram-type");assert_eq!(refusal.reason,"production memory reclock requires DDR3");
 // A new highest perf level must change the target, without constants.
 r[0x909]=0x11;r[0x13a..0x13c].copy_from_slice(&1450u16.to_le_bytes());
 assert_eq!(production_rom::target(&r,4).unwrap().gpc_khz,1450000);
 r.truncate(0x901);assert!(production_rom::target(&r,4).is_err());
}
''' % tuple(str(ROOT/'kernel/src/gpu/gk208'/n) for n in ['edid.rs','modeset.rs','perf.rs','production_rom.rs']))
   priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
   subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
   subprocess.run([str(p/'check')],check=True)
if __name__=='__main__':unittest.main()
