"""A valid large native submission preserves draws without one request per rect."""
from pathlib import Path
import subprocess,sys,tempfile,unittest
ROOT=Path(__file__).resolve().parents[1]
class Batch(unittest.TestCase):
    def test_normal_submit_batches_ten_thousand_rectangles_and_preserves_packets(self):
        core=(ROOT/'kernel/src/gpu/mod.rs').read_text()
        start=core.index('                native_probe_mask = frame.probe_mask;')
        stop=core.index('                #[cfg(feature',start)
        code=core[start:stop]
        fixture=(ROOT/'tests/gt710_draw_encode_test.rs').read_text()
        fixture=fixture.replace('"../kernel/', '"'+str(ROOT/'kernel')+'/')
        fixture+=r'''
fn core_prepare(mut frame:encode::Frame)->Result<encode::Frame,i64>{
 const EINVAL:i64=22;let native_probe_mask;
 CODE
 assert_eq!(native_probe_mask,0);Ok(frame)
}
#[test]fn normal_submit_batches_valid_ten_thousand_rectangles(){
 let mut words=Vec::new();stream::encode::begin(&mut words,1);
 for c in 0..100{stream::encode::draw(&mut words,pipeline::SOLID,0xff000000|c,&[[0,0,16,16];100]);}
 let mut expected=encode::translate(words.as_slice(),&View(format::B8G8R8A8)).unwrap();
 let unbatched=expected.chunks.len();expected.compact().unwrap();expected.relocate(0x3_0000_0000).unwrap();
 let packets_before:Vec<_>=expected.chunks.iter().flat_map(|c|packets(&c.words)).filter(|(m,_)|*m!=0x17bc).collect();
 let mut actual=core_prepare(encode::translate(words.as_slice(),&View(format::B8G8R8A8)).unwrap()).unwrap();
 assert!(actual.chunks.len()<unbatched,"normal SUBMIT retained one native request for each of {unbatched} rectangles");
 assert!(actual.chunks.iter().all(|c|c.words.len()<=1400));actual.relocate(0x3_0000_0000).unwrap();
 let packets_after:Vec<_>=actual.chunks.iter().flat_map(|c|packets(&c.words)).filter(|(m,_)|*m!=0x17bc).collect();
 assert_eq!(packets_before,packets_after,"batching lost draws, state or relocations");
}
'''.replace('CODE',code)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'batch.rs').write_text(fixture)
            subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings','--extern',f'gpu_abi={p/"libgpu_abi.rlib"}',str(p/'batch.rs'),'-o',str(p/'batch')],check=True)
            subprocess.run([str(p/'batch'),'normal_submit_batches_valid_ten_thousand_rectangles','--exact','--nocapture'],check=True)
if __name__=='__main__':unittest.main()
