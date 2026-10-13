//! Atomic replacement of only the mutable open-description status bits.
use core::sync::atomic::{AtomicU32,Ordering};
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
}
