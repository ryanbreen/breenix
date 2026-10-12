//! Execute production typed-node creation against an in-memory block device.
use crate::{
    block::{BlockDevice, BlockError},
    ext2::*,
};
use std::sync::{Arc, Mutex};
struct Disk(Arc<Mutex<Vec<u8>>>);
impl BlockDevice for Disk {
    fn read_block(&self, b: u64, out: &mut [u8]) -> Result<(), BlockError> {
        let i = b as usize * 512;
        out.copy_from_slice(&self.0.lock().unwrap()[i..i + out.len()]);
        Ok(())
    }
    fn write_block(&self, b: u64, input: &[u8]) -> Result<(), BlockError> {
        let i = b as usize * 512;
        self.0.lock().unwrap()[i..i + input.len()].copy_from_slice(input);
        Ok(())
    }
    fn block_size(&self) -> usize {
        512
    }
    fn num_blocks(&self) -> u64 {
        4096
    }
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
}
pub(super) fn fixture() -> (Ext2Fs, Arc<Mutex<Vec<u8>>>) {
    let bytes = Arc::new(Mutex::new(vec![0; 512 * 4096]));
    let mut sb = Ext2Superblock::from_bytes(&[0; 1024]).unwrap();
    sb.s_blocks_count = 512;
    sb.s_free_blocks_count = 480;
    sb.s_inodes_count = 128;
    sb.s_free_inodes_count = 118;
    sb.s_log_block_size = 2;
    sb.s_blocks_per_group = 512;
    sb.s_inodes_per_group = 128;
    let mut group: Ext2BlockGroupDesc = unsafe { core::mem::zeroed() };
    group.bg_inode_table = 8;
    group.bg_block_bitmap = 3;
    group.bg_inode_bitmap = 4;
    group.bg_free_blocks_count = 480;
    group.bg_free_inodes_count = 118;
    {
        let mut data = bytes.lock().unwrap();
        data[4 * 4096] = 0xff;
        data[4 * 4096 + 1] = 3;
    }
    let mut root = Ext2Inode::new_directory(0o777);
    root.i_size = 4096;
    root.i_blocks = 8;
    root.i_block[0] = 16;
    root.i_atime = 0;
    root.i_mtime = 0;
    root.i_ctime = 0;
    let fs = Ext2Fs::host_new(Box::new(Disk(bytes.clone())), sb, vec![group]);
    root.write_to(fs.device.as_ref(), 2, &fs.superblock, &fs.block_groups)
        .unwrap();
    let mut data = vec![0; 4096];
    data[..4].copy_from_slice(&2u32.to_le_bytes());
    data[4..6].copy_from_slice(&12u16.to_le_bytes());
    data[6] = 1;
    data[7] = 2;
    data[8] = b'.';
    data[12..16].copy_from_slice(&2u32.to_le_bytes());
    data[16..18].copy_from_slice(&4084u16.to_le_bytes());
    data[18] = 2;
    data[19] = 2;
    data[20..22].copy_from_slice(b"..");
    fs.write_directory_for_host(2, &data).unwrap();
    (fs, bytes)
}
#[test]
fn fifo_creation_persists_type_identity_ownership_and_timestamps() {
    let (mut fs, bytes) = fixture();
    let ino = fs
        .create_node(2, "fifo", 0o010640, 70000, 80000, 0)
        .unwrap();
    fs.sync().unwrap();
    let snapshot = bytes.lock().unwrap().clone();
    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.file_type(), FileType::Fifo);
    assert_eq!(inode.permissions(), 0o640);
    assert_eq!((inode.uid(), inode.gid()), (70000, 80000));
    assert_eq!(inode.size(), 0);
    let times = (inode.i_atime, inode.i_mtime, inode.i_ctime);
    assert_eq!(times, (1, 1, 1));
    let parent = fs.read_inode(2).unwrap();
    let times = (parent.i_atime, parent.i_mtime, parent.i_ctime);
    assert_eq!(times, (0, 1, 1));
    let entries = fs.read_directory(&parent).unwrap();
    let entry = find_entry(&entries, "fifo").unwrap();
    assert_eq!(entry.inode, ino);
    assert_eq!(entry.file_type, EXT2_FT_FIFO);
    assert!(!is_directory_empty(&entries));
    assert_eq!(
        fs.create_node(2, "fifo", 0o100600, 1, 2, 0),
        Err("File already exists")
    );
    assert_eq!(&*bytes.lock().unwrap(), &snapshot);
    let reopened = Ext2Fs::host_new(
        Box::new(Disk(Arc::new(Mutex::new(snapshot)))),
        fs.superblock,
        fs.block_groups,
    );
    let inode = reopened.read_inode(ino).unwrap();
    assert_eq!(inode.file_type(), FileType::Fifo);
    assert_eq!(
        find_entry(
            &reopened
                .read_directory(&reopened.read_inode(2).unwrap())
                .unwrap(),
            "fifo"
        )
        .unwrap()
        .inode,
        ino
    );
}
#[test]
fn ext2_special_nodes_store_linux_device_encoding_and_directory_types() {
    let (mut fs, _) = fixture();
    for (name, kind, dt) in [
        ("regular", 0o100000, 1),
        ("fifo", 0o010000, 5),
        ("char", 0o020000, 3),
        ("block", 0o060000, 4),
        ("socket", 0o140000, 6),
    ] {
        let ino = fs
            .create_node(2, name, kind | 0o600, 1, 2, 0x12345678)
            .unwrap();
        let data = fs.read_directory(&fs.read_inode(2).unwrap()).unwrap();
        assert_eq!(find_entry(&data, name).unwrap().file_type, dt);
        let inode = fs.read_inode(ino).unwrap();
        let mode = inode.i_mode;
        assert_eq!(mode, kind | 0o600);
        if kind == 0o020000 || kind == 0o060000 {
            let dev = inode.i_block[1];
            assert_eq!(dev, 0x12345678);
            assert_eq!(create_node::node_device(&inode), 0x12345678);
        }
    }
}

#[test]
fn fifo_entry_persists_in_an_existing_indirect_directory_block() {
    let (mut fs, bytes) = fixture();
    let mut root = fs.read_inode(2).unwrap();
    root.i_size = 13 * 4096;
    root.i_blocks = 14 * 8;
    for index in 0..12 {
        root.i_block[index] = 20 + index as u32;
    }
    root.i_block[12] = 17;
    bytes.lock().unwrap()[17 * 4096..17 * 4096 + 4].copy_from_slice(&18u32.to_le_bytes());
    fs.write_inode(2, &root).unwrap();
    let mut data = vec![0; 13 * 4096];
    for block in 0..12 {
        for index in 0..16 {
            let offset = block * 4096 + index * 264;
            let len = if index == 15 { 136 } else { 264 };
            let names = if index == 15 { 128 } else { 255 };
            data[offset..offset + 4].copy_from_slice(&3u32.to_le_bytes());
            data[offset + 4..offset + 6].copy_from_slice(&(len as u16).to_le_bytes());
            data[offset + 6] = names as u8;
            data[offset + 7] = EXT2_FT_REG_FILE;
            data[offset + 8..offset + 8 + names].fill(b'x');
        }
    }
    data[12 * 4096 + 4..12 * 4096 + 6].copy_from_slice(&4096u16.to_le_bytes());
    fs.write_directory_for_host(2, &data).unwrap();
    let ino = fs.create_node(2, "fifo", 0o010600, 1, 2, 0).unwrap();
    fs.sync().unwrap();
    let data = fs.read_directory(&fs.read_inode(2).unwrap()).unwrap();
    assert_eq!(dir::find_inode(&data, "fifo"), Some(ino));
    let entry = dir::raw_entries(&data)
        .find(|entry| entry.name == b"fifo")
        .unwrap();
    assert_eq!(entry.offset, 12 * 4096);
    let snapshot = bytes.lock().unwrap();
    assert_eq!(
        u32::from_le_bytes(snapshot[18 * 4096..18 * 4096 + 4].try_into().unwrap()),
        ino
    );
}

#[test]
fn old_and_new_device_encodings_decode_to_linux_rdev() {
    for dev in [0, 0x0103, 0xffff, 0x10000, 0x12345678, u32::MAX] {
        for kind in [0o020000, 0o060000] {
            let inode = create_node::node_inode(kind, 0, 0, dev);
            assert_eq!(create_node::node_device(&inode), dev as u64);
        }
    }
}

#[test]
fn failed_directory_publication_restores_name_and_releases_inode() {
    struct FailingDisk {
        bytes: Arc<Mutex<Vec<u8>>>,
        sector: u64,
        landed: bool,
        armed: Mutex<bool>,
    }
    impl BlockDevice for FailingDisk {
        fn read_block(&self, b: u64, out: &mut [u8]) -> Result<(), BlockError> {
            Disk(self.bytes.clone()).read_block(b, out)
        }
        fn write_block(&self, b: u64, data: &[u8]) -> Result<(), BlockError> {
            let fail = {
                let mut armed = self.armed.lock().unwrap();
                let fail = b == self.sector && *armed;
                if fail {
                    *armed = false;
                }
                fail
            };
            if !fail || self.landed {
                Disk(self.bytes.clone()).write_block(b, data)?;
            }
            if fail {
                Err(BlockError::IoError)
            } else {
                Ok(())
            }
        }
        fn block_size(&self) -> usize {
            512
        }
        fn num_blocks(&self) -> u64 {
            4096
        }
        fn flush(&self) -> Result<(), BlockError> {
            Ok(())
        }
    }
    for sector in 16 * 8..17 * 8 {
        for landed in [false, true] {
            let (mut fs, bytes) = fixture();
            let bitmap = bytes.lock().unwrap()[4 * 4096..5 * 4096].to_vec();
            let free = fs.block_groups[0].bg_free_inodes_count;
            fs.device = Box::new(FailingDisk {
                bytes: bytes.clone(),
                sector,
                landed,
                armed: Mutex::new(true),
            });
            assert!(fs.create_node(2, "fifo", 0o010600, 0, 0, 0).is_err());
            let parent = fs.read_inode(2).unwrap();
            assert!(find_entry(&fs.read_directory(&parent).unwrap(), "fifo").is_none());
            assert_eq!(&bytes.lock().unwrap()[4 * 4096..5 * 4096], &bitmap);
            let after = fs.block_groups[0].bg_free_inodes_count;
            assert_eq!(after, free);
        }
    }
}
