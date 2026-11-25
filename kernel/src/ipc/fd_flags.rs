//! Atomic replacement of only the mutable open-description status bits.
use core::sync::atomic::{AtomicU32,Ordering};

pub const FD_CLOEXEC: u32 = 1;
pub const FD_CLOFORK: u32 = 2;
pub const O_CLOEXEC: u32 = 0x0008_0000;
pub const O_CLOFORK: u32 = 0x0200_0000;

pub(crate) fn from_open_flags(open: u32) -> u32 {
    let mut descriptor = 0;
    if open & O_CLOEXEC != 0 {
        descriptor |= FD_CLOEXEC;
    }
    if open & O_CLOFORK != 0 {
        descriptor |= FD_CLOFORK;
    }
    descriptor
}

pub(crate) fn inherited_on_fork(descriptor: u32) -> bool {
    descriptor & FD_CLOFORK == 0
}

pub(crate) fn replace(word:&AtomicU32,flags:u32,mask:u32)->u32 {
    let mut old=word.load(Ordering::Acquire);
    loop {
        let new=(old & !mask) | (flags & mask);
        match word.compare_exchange_weak(old,new,Ordering::AcqRel,Ordering::Acquire) {
            Ok(previous)=>return previous, Err(actual)=>old=actual,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn swap_returns_prior_word_and_keeps_fixed_access_bits() {
        let word=AtomicU32::new(2|0x800);
        assert_eq!(replace(&word,0x400,0xc00),2|0x800);
        assert_eq!(word.load(Ordering::Acquire),2|0x400);
        assert_eq!(replace(&word,0x10000,0xc00),2|0x400);
        assert_eq!(word.load(Ordering::Acquire),2);
    }

    #[test]
    fn dup3_flags_exclude_only_close_on_fork_descriptors_from_fork() {
        let descriptor = from_open_flags(0x0208_0000 | 2);
        assert_eq!(descriptor, 3);
        assert!(!inherited_on_fork(descriptor));
        assert!(inherited_on_fork(from_open_flags(0x0008_0000)));
        assert!(inherited_on_fork(from_open_flags(0)));
    }
}
