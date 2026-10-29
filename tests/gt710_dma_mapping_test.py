"""Run production DMA admission repeatedly without leaking register mappings."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DmaMappings(unittest.TestCase):
    def test_reuse_mappings_but_resample_live_translation_status(self):
        source = (ROOT / 'kernel/src/gpu/gk208/system_dma.rs').read_text()
        admission = source[source.index('static PLATFORM:'):source.index('pub struct Pages {')]
        harness = r'''
extern crate alloc;
use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};
mod spin {
 pub struct Once<T>(std::sync::OnceLock<T>);
 impl<T> Once<T> {
  pub const fn new()->Self {Self(std::sync::OnceLock::new())}
  pub fn get(&self)->Option<&T>{self.0.get()}
  pub fn call_once(&self,f:impl FnOnce()->T)->&T{self.0.get_or_init(f)}
 }
}
#[path="POLICY"] pub mod dma_policy;
static MAPS: AtomicUsize = AtomicUsize::new(0);
static REGISTERS: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());
mod memory {
 pub fn map_mmio(base:u64, bytes:usize)->Result<usize,&'static str>{
  assert_eq!(bytes,4096);
  super::MAPS.fetch_add(1,super::Ordering::Relaxed);
  let address=Box::into_raw(Box::new([0u32;1024])) as usize;
  super::REGISTERS.lock().unwrap().push((base,address));
  Ok(address)
 }
}
mod arch_impl {pub mod x86_64 {pub mod acpi {
 pub fn dma_remapping_table(rsdp:Option<u64>,offset:u64)->Result<Vec<u8>,&'static str>{
  assert_eq!((rsdp,offset),(Some(42),99));
  let mut table=vec![0u8;80];table[..4].copy_from_slice(b"DMAR");
  table[4..8].copy_from_slice(&80u32.to_le_bytes());
  for (i,base) in [0x1000u64,0x2000].into_iter().enumerate(){
   let at=48+i*16;table[at+2..at+4].copy_from_slice(&16u16.to_le_bytes());
   table[at+8..at+16].copy_from_slice(&base.to_le_bytes());
  }
  table[9]=0u8.wrapping_sub(table.iter().fold(0u8,|a,b|a.wrapping_add(*b)));
  Ok(table)
 }
}}}
// ADMISSION
fn status(unit:u64,value:u32){
 for &(base,address) in REGISTERS.lock().unwrap().iter(){if base==unit {
  // SAFETY: mappings are owned arrays, and callers have joined sampling threads.
  unsafe {((address+0x1c) as *mut u32).write_volatile(value);}
 }}
}
fn main(){
 assert!(translation_status().is_err());
 set_platform(Some(42),99);
 assert_eq!(translation_status().unwrap(),[0,0]);
 std::thread::scope(|scope|{for _ in 0..8{scope.spawn(||{
  for _ in 0..2500{assert_eq!(translation_status().unwrap(),[0,0]);}
 });}});
 assert_eq!(MAPS.load(Ordering::Relaxed),2,"DMA allocation remapped the DRHD register pages");
 status(0x2000,1<<31);
 assert_eq!(translation_status().unwrap_err(),"active DMA translation needs a platform mapping");
 status(0x2000,0);status(0x1000,u32::MAX);
 assert_eq!(translation_status().unwrap_err(),"unreadable VT-d global status");
 status(0x1000,0);assert_eq!(translation_status().unwrap(),[0,0]);
 assert_eq!(MAPS.load(Ordering::Relaxed),2);
}
'''.replace('POLICY', str(ROOT / 'kernel/src/gpu/gk208/dma_policy.rs')).replace('// ADMISSION', admission)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'admission.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'admission.rs'), '-o', str(path / 'admission')], check=True)
            subprocess.run([str(path / 'admission')], check=True)


if __name__ == '__main__':
    unittest.main()
