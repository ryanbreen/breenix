"""Record actual mmap requests: empty evidence slots must not populate pixels."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
class Mapping(unittest.TestCase):
    def test_payload_is_mapped_only_when_used(self):
        rel = 'libs/libuitest/src/arena.rs'
        old = os.environ.get('UITEST_OLD_COMMIT')
        text = subprocess.check_output(['git','show',f'{old}:{rel}'],cwd=ROOT,text=True) if old else (ROOT/rel).read_text()
        text = text.replace('    fn mmap(', '    #[link_name="mmap"]\n    fn system_mmap(')
        text += r'''
static MAPS: std::sync::Mutex<Vec<(usize,i64)>> = std::sync::Mutex::new(Vec::new());
unsafe fn mmap(addr:*mut u8,len:usize,prot:i32,flags:i32,fd:i32,offset:i64)->*mut u8 {
    MAPS.lock().unwrap().push((len,offset));system_mmap(addr,len,prot,flags,fd,offset)
}
#[test]fn deferred_pixels(){
 let root=std::env::temp_dir().join(format!("arena-mapping-{}",std::process::id()));
 std::fs::create_dir(&root).unwrap();let path=root.join("arena");
 let layout=Layout::for_mode(2560,1600).unwrap();
 let mut writer=Arena::create(&path,layout).unwrap();let reader=Arena::open(&path).unwrap();
 let before=MAPS.lock().unwrap().clone();
 assert_eq!(before, [Slot::Screen,Slot::Frame,Slot::Tree].repeat(2).iter().map(|s|(HEADER,layout.header_offset(*s) as i64)).collect::<Vec<_>>(),"startup eagerly populated unused pixel slots");
 assert!(matches!(reader.read(Slot::Screen).unwrap(),SlotRead::Free));
 assert_eq!(*MAPS.lock().unwrap(),before);
 let mut slot=writer.begin(Slot::Frame).unwrap();slot.bytes_mut()[..4].copy_from_slice(&[1,2,3,4]);
 assert!(matches!(reader.read(Slot::Frame).unwrap(),SlotRead::Partial(_)));
 slot.publish(Record::default(),4).unwrap();
 let SlotRead::Final{bytes,..}=reader.read(Slot::Frame).unwrap() else {panic!("not published")};assert_eq!(bytes,[1,2,3,4]);
 let maps=MAPS.lock().unwrap();assert_eq!(&maps[before.len()..],&[(layout.capacity(Slot::Frame),layout.data_offset(Slot::Frame) as i64),(PAGE,layout.data_offset(Slot::Frame) as i64)]);
 drop(reader);drop(writer);std::fs::remove_dir_all(root).unwrap();
}
'''
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)
            def run(args):
                if sys.platform=='darwin': args=['taskpolicy','-b',*args]
                r=subprocess.run(args,cwd=ROOT,capture_output=True,text=True)
                self.assertEqual(r.returncode,0,r.stdout+r.stderr)
            run(['rustc','--edition=2021','--crate-name','libpixcheck','--crate-type=rlib','-Dwarnings','libs/libpixcheck/src/lib.rs','-o',str(path/'liblibpixcheck.rlib')])
            (path/'arena.rs').write_text(text)
            run(['rustc','--edition=2021','--test','-Dwarnings',str(path/'arena.rs'),'--extern',f'libpixcheck={path}/liblibpixcheck.rlib','-o',str(path/'check')])
            run([str(path/'check')])
if __name__=='__main__': unittest.main()
