use super::*;
use crate::block::{BlockDevice, BlockError};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request { Read, Write, Flush }

struct CountingDisk {
    inner: Box<dyn BlockDevice>,
    requests: Arc<Mutex<Vec<Request>>>,
}
impl BlockDevice for CountingDisk {
    fn read_block(&self, block: u64, out: &mut [u8]) -> Result<(), BlockError> {
        self.requests.lock().unwrap().push(Request::Read);
        self.inner.read_block(block, out)
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        self.inner.write_block(block, bytes)?;
        self.requests.lock().unwrap().push(Request::Write);
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> {
        self.inner.flush()?;
        self.requests.lock().unwrap().push(Request::Flush);
        Ok(())
    }
    fn block_size(&self) -> usize { self.inner.block_size() }
    fn num_blocks(&self) -> u64 { self.inner.num_blocks() }
}

fn counted_mount(mut fs: Ext2Fs) -> (Ext2Fs, Arc<Mutex<Vec<Request>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    fs.device = Box::new(mount_device::MountDevice::new(
        Box::new(CountingDisk { inner: fs.device, requests: requests.clone() }),
        Arc::new(core::sync::atomic::AtomicBool::new(false)), true));
    (fs, requests)
}

#[test]
fn ext2_reread_of_a_directory_issues_no_device_reads() {
    let (mut fs, data, _) = reclaim_checks::directory_fixture();
    // A 1 MiB directory fits the shared cache's minimum memory budget.
    let mut inode = fs.read_inode(2).unwrap();
    inode.i_size = 256 * 4096;
    inode.i_blocks = 257 * 8;
    for i in 0..12 { inode.i_block[i] = 100 + i as u32; }
    inode.i_block[12] = 40;
    {
        let mut bytes = data.lock().unwrap();
        for i in 12..256 {
            let at = 40 * 4096 + (i - 12) * 4;
            bytes[at..at + 4].copy_from_slice(&(100 + i as u32).to_le_bytes());
        }
        for i in 0..256 {
            let mut dir = vec![0; 4096];
            dir[4..6].copy_from_slice(&4096u16.to_le_bytes());
            dir::add_directory_entry(&mut dir, 2, &format!("entry-{i}"), dir::EXT2_FT_DIR).unwrap();
            bytes[(100+i)*4096..(101+i)*4096].copy_from_slice(&dir);
        }
    }
    fs.write_inode(2, &inode).unwrap();
    let (fs, requests) = counted_mount(fs);
    let first = fs.read_directory(&inode).unwrap();
    assert!(requests.lock().unwrap().contains(&Request::Read));
    requests.lock().unwrap().clear();
    assert_eq!(fs.read_directory(&inode).unwrap(), first);
    assert_eq!(requests.lock().unwrap().iter().filter(|r| **r == Request::Read).count(), 0);
}

#[test]
fn ext2_fsync_flush_follows_the_files_last_write() {
    let (fs, _, _) = reclaim_checks::directory_fixture();
    let (mut fs, requests) = counted_mount(fs);
    let ino = fs.create_node(2, "written", 0o100600, 0, 0, 0).unwrap();
    requests.lock().unwrap().clear();
    assert_eq!(fs.write_file_range(ino, 0, b"durable bytes").unwrap(), 13);
    assert_eq!(requests.lock().unwrap().last(), Some(&Request::Write));
    fs.sync().unwrap();
    let requests = requests.lock().unwrap();
    let last_write = requests.iter().rposition(|r| *r == Request::Write).unwrap();
    let flush = requests.iter().position(|r| *r == Request::Flush).unwrap();
    assert!(flush > last_write, "{requests:?}");
    assert_eq!(flush, requests.len() - 1);
}

#[test]
fn remount_read_only_preserves_cached_bytes_and_refuses_backend_writes() {
    let (fs, _, _) = reclaim_checks::directory_fixture();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let disk = mount_device::MountDevice::new(
        Box::new(CountingDisk { inner: fs.device, requests: requests.clone() }),
        Arc::new(core::sync::atomic::AtomicBool::new(false)), true);
    let mut before = [0; 512];
    disk.read_block(0, &mut before).unwrap();
    disk.remount_read_only();
    assert_eq!(disk.write_block(0, &[0xff; 512]), Err(BlockError::IoError));
    assert_eq!(disk.write_blocks(0, 2, &[0xff; 1024]), Err(BlockError::IoError));
    let mut after = [0; 512];
    disk.read_block(0, &mut after).unwrap();
    assert_eq!(before, after);
    assert_eq!(*requests.lock().unwrap(), vec![Request::Read]);
}
