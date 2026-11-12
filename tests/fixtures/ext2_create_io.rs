use std::{collections::BTreeMap, sync::{Arc, Mutex}};
use block::{BlockDevice, BlockError};
use fs::ext2::*;

#[derive(Default)]
struct State {
    sectors: BTreeMap<u64, [u8; 512]>,
    reads: usize,
    read_sectors: usize,
    writes: Vec<u64>,
    // The write lands before reporting the injected one-shot error.
    fail: Option<u64>,
}
struct Disk(Arc<Mutex<State>>);
impl BlockDevice for Disk {
    fn read_block(&self, start: u64, out: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(start, 1, out)
    }
    fn read_blocks(&self, start: u64, count: usize, out: &mut [u8]) -> Result<(), BlockError> {
        let mut s = self.0.lock().unwrap();
        s.reads += 1;
        s.read_sectors += count;
        for (offset, bytes) in out[..count*512].chunks_exact_mut(512).enumerate() {
            bytes.copy_from_slice(s.sectors.get(&(start+offset as u64)).unwrap_or(&[0;512]));
        }
        Ok(())
    }
    fn write_block(&self, start: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let mut s = self.0.lock().unwrap();
        s.sectors.insert(start, bytes[..512].try_into().unwrap());
        s.writes.push(start);
        if s.fail == Some(start) { s.fail = None; Err(BlockError::IoError) } else { Ok(()) }
    }
    // Match ARM's sector-at-a-time write backend (the trait default).
    fn block_size(&self) -> usize { 512 }
    fn num_blocks(&self) -> u64 { u64::MAX }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}
fn reset(state: &Arc<Mutex<State>>) {
    let mut s=state.lock().unwrap(); s.reads=0; s.read_sectors=0; s.writes.clear();
}
fn fixture(groups: u32) -> (Ext2Fs, Arc<Mutex<State>>) {
    let mut sb=[0;1024];
    for (offset,value) in [(0,groups*2000u32),(4,groups*8192),(16,1989),(20,1),
                          (32,8192),(40,2000),(76,1),(84,11)] {
        sb[offset..offset+4].copy_from_slice(&value.to_le_bytes());
    }
    sb[56..58].copy_from_slice(&0xef53u16.to_le_bytes());
    sb[88..90].copy_from_slice(&256u16.to_le_bytes());
    let superblock=Ext2Superblock::from_bytes(&sb).unwrap();
    let mut descriptors=Vec::new();
    for group in 0..groups {
        let mut bytes=[0u8;32];
        for (offset,value) in [(0,group*8192+100),(4,group*8192+101),(8,group*8192+102)] {
            bytes[offset..offset+4].copy_from_slice(&value.to_le_bytes());
        }
        if group==groups-1 { bytes[14..16].copy_from_slice(&1989u16.to_le_bytes()); }
        // SAFETY: the descriptor is a packed 32-byte disk structure.
        descriptors.push(unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const Ext2BlockGroupDesc) });
    }
    let state=Arc::new(Mutex::new(State::default()));
    let disk=Disk(state.clone());
    // Reserved inodes and any earlier full groups are not allocation candidates.
    let mut bitmap=[0;1024]; bitmap[0]=255; bitmap[1]=7;
    file::write_ext2_block(&disk,(groups-1)*8192+101,1024,&bitmap).unwrap();
    let mut parent=Ext2Inode::new_directory(0o755);
    parent.i_size=1024; parent.i_blocks=2; parent.i_block[0]=700;
    parent.write_to(&disk,2,&superblock,&descriptors).unwrap();
    let mut entries=vec![0;1024]; entries[4..6].copy_from_slice(&1024u16.to_le_bytes());
    dir::add_directory_entry(&mut entries,2,".",EXT2_FT_DIR).unwrap();
    dir::add_directory_entry(&mut entries,2,"..",EXT2_FT_DIR).unwrap();
    file::write_ext2_block(&disk,700,1024,&entries).unwrap();
    superblock.write_to(&disk).unwrap();
    Ext2BlockGroupDesc::write_table(&disk,&superblock,&descriptors).unwrap();
    reset(&state);
    (Ext2Fs{device:Box::new(disk),superblock,block_groups:descriptors},state)
}
fn main() {
    for groups in [1,39,257] {
        let (mut fs,state)=fixture(groups);
        for past in 0..16 {
            reset(&state);
            fs.create_node(2,format!("arena-{past}"),0o100600,0,0,0).unwrap();
            let s=state.lock().unwrap();
            println!("groups={groups} past={past} reads={} read_sectors={} write_sectors={}",s.reads,s.read_sectors,s.writes.len());
            let table=4+((groups-1)/32) as u64*2;
            // Order is bitmap, new inode, name, parent, changed counts, superblock.
            let bitmap=((groups-1)*8192+101) as u64*2;
            assert_eq!(s.writes[0..2],[bitmap,bitmap+1]);
            assert_eq!(s.writes[3..5],[1400,1401]);
            assert_eq!(&s.writes[s.writes.len()-2..],&[2,3]);
            assert_eq!(s.reads,8,"create unexpectedly reread group bitmaps");
            assert_eq!(s.read_sectors,10);
            assert_eq!(s.writes.len(),10,"create writes grow with unrelated groups");
            assert_eq!(s.writes[6..8],[table,table+1]);
        }
        // Counts publication can fail after writing either the changed table
        // block or the superblock. Undo must restore the entire namespace image.
        for failed in [4+((groups-1)/32) as u64*2+1,3] {
            let before=state.lock().unwrap().sectors.clone();
            state.lock().unwrap().fail=Some(failed);
            assert!(fs.create_node(2,"refused",0o100600,0,0,0).is_err());
            let after=state.lock().unwrap().sectors.clone();
            for sector in before.keys().chain(after.keys()) {
                assert_eq!(after.get(sector).unwrap_or(&[0;512]),before.get(sector).unwrap_or(&[0;512]),"undo changed sector {sector}");
            }
            assert_eq!(fs.read_directory(&fs.read_inode(2).unwrap()).unwrap().len(),1024);
        }
    }
}
