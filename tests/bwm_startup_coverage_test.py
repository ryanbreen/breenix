"""The first atlas batch covers future clock ticks and surviving window chrome."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class StartupCoverage(unittest.TestCase):
    def test_clock_and_surviving_titles_share_the_first_batch(self):
        source=(ROOT/'userspace/programs/src/bwm.rs').read_text()
        body=item(source,'            if let Some(font) = font.as_mut() {')
        helper=ROOT/'userspace/programs/src/bwm/startup_prepare.rs'
        run_rust(self, r'''
use std::collections::BTreeSet;
#[path="@HELPER@"] pub mod startup_prepare;
struct Ui{scale:u32}fn ui()->Ui{Ui{scale:2}}fn ttf_font_size()->f32{14.0}
struct Font{seen:BTreeSet<u8>,calls:usize}
impl Font{fn prepare_subpixel(&mut self,texts:&[&[u8]],size:f32)->Result<(),String>{
 assert_eq!(size,28.0);self.calls+=1;
 self.seen.extend(texts.iter().flat_map(|s|s.iter()).copied());Ok(())
}}
#[test]fn first_batch(){
 let clock_text=*b"00:00:00 ET";
 let startup_titles=vec![b"Terminal".as_slice(),b"WM client 11".as_slice()];
 assert_eq!(startup_titles.len(),2);
 let mut font=Some(Font{seen:BTreeSet::new(),calls:0});
 @BODY@
 let font=font.unwrap();assert_eq!(font.calls,1);
 for b in b"0123456789: ETBreenixx-TerminalWM client 11"{
  assert!(font.seen.contains(b),"cold startup coverage for {}",*b as char);
 }
}
'''.replace('@HELPER@',str(helper)).replace('@BODY@',body))
if __name__=='__main__':unittest.main()
