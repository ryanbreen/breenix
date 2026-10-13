//! Deferred FIFO times. Namespace changes supersede older transfer ctime;
//! a transfer concurrent with metadata I/O keeps its newer pending value.
use super::Ext2Inode;
use core::sync::atomic::{AtomicU64, Ordering};

pub(super) fn encode(seconds: u32) -> u64 {
    u64::from(seconds) + 1
}

pub(super) fn apply(inode: &mut Ext2Inode, atime: u64, mtime: u64, ctime: u64) {
    if atime != 0 {
        inode.i_atime = (atime - 1) as u32;
    }
    if mtime != 0 {
        inode.i_mtime = (mtime - 1) as u32;
    }
    if ctime != 0 {
        inode.i_ctime = (ctime - 1) as u32;
    }
}

pub(super) fn write_metadata<T>(
    pending_ctime: &AtomicU64,
    write: impl FnOnce() -> Result<T, &'static str>,
) -> Result<T, &'static str> {
    let previous = pending_ctime.swap(0, Ordering::AcqRel);
    let result = write();
    if result.is_err() {
        // Do not replace a concurrent transfer's time, including after a
        // backwards clock adjustment, with the failed operation's old cache.
        let _ = pending_ctime.compare_exchange(0, previous, Ordering::AcqRel, Ordering::Acquire);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn times(inode: &Ext2Inode) -> (u32, u32, u32) {
        (inode.i_atime, inode.i_mtime, inode.i_ctime)
    }
    #[test]
    fn namespace_ctime_survives_deferred_transfer_writeback() {
        let mut inode = Ext2Inode::new_regular_file(0o600, 0, 0);
        let pending = AtomicU64::new(encode(3));
        write_metadata(&pending, || {
            inode.i_ctime = 10;
            Ok(())
        })
        .unwrap();
        apply(
            &mut inode,
            encode(2),
            encode(3),
            pending.swap(0, Ordering::AcqRel),
        );
        assert_eq!(times(&inode), (2, 3, 10));
        write_metadata(&pending, || {
            inode.i_ctime = 11;
            // A later transfer may see a clock moved backwards.
            pending.store(encode(4), Ordering::Release);
            Ok(())
        })
        .unwrap();
        apply(&mut inode, 0, encode(4), pending.swap(0, Ordering::AcqRel));
        assert_eq!(times(&inode), (2, 4, 4));
    }
    #[test]
    fn failed_metadata_io_restores_old_time_without_overwriting_a_transfer() {
        let pending = AtomicU64::new(encode(3));
        assert_eq!(
            write_metadata(&pending, || Err::<(), _>("disk failure")),
            Err("disk failure")
        );
        assert_eq!(pending.load(Ordering::Acquire), encode(3));
        assert_eq!(
            write_metadata(&pending, || {
                pending.store(encode(2), Ordering::Release);
                Err::<(), _>("disk failure")
            }),
            Err("disk failure")
        );
        assert_eq!(pending.load(Ordering::Acquire), encode(2));
    }
    #[test]
    fn epoch_zero_is_a_real_pending_time() {
        let mut inode = Ext2Inode::new_regular_file(0o600, 0, 0);
        apply(&mut inode, encode(0), encode(0), encode(0));
        assert_eq!(times(&inode), (0, 0, 0));
    }
}
