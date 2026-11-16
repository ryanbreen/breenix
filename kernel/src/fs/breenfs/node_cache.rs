//! Read-only verified node store (breenfs design sections 1.2 and 2.2).
use super::{
    btree::{Error, NodeStore},
    format::Superblock,
};
use crate::block::{
    cache::{BufferCache, Wait},
    BlockDevice,
};
use alloc::vec::Vec;
use core::marker::PhantomData;

pub struct CachedNodeStore<'a, W: Wait> {
    scope: PhantomData<(&'a BufferCache<W>, &'a dyn BlockDevice)>,
}

impl<'a, W: Wait> CachedNodeStore<'a, W> {
    pub fn new(_: &'a BufferCache<W>, _: &'a dyn BlockDevice, _: &Superblock) -> Self {
        Self { scope: PhantomData }
    }
}

impl<W: Wait> NodeStore for CachedNodeStore<'_, W> {
    fn get(&self, _: u64) -> Result<Vec<u8>, Error> {
        Err(Error::Store)
    }
    fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), Error> {
        Err(Error::Store)
    }
    fn alloc(&mut self) -> Result<u64, Error> {
        Err(Error::Store)
    }
}
