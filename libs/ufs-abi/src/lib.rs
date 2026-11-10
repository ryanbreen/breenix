#![no_std]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Op { Lookup=1, Getattr, Setattr, Readdir, Open, Read, Write, Create, Mkdir, Unlink, Rmdir, Rename, Fsync, Statfs, Release }
pub const MAX_DATA: usize = 65536;
pub const REQUEST_HEADER: usize = 22;
pub const REPLY_HEADER: usize = 16;
pub const MAX_MESSAGE: usize = REQUEST_HEADER + MAX_DATA;
pub const NAME_MAX: usize = 255;
#[derive(Debug, PartialEq, Eq)]
pub struct Request<'a> { pub id: u64, pub op: Op, pub node: u64, pub payload: &'a [u8] }
#[derive(Debug, PartialEq, Eq)]
pub struct Reply<'a> { pub id: u64, pub errno: i32, pub payload: &'a [u8] }
#[derive(Debug, PartialEq, Eq)]
pub struct Error;
impl Request<'_> { pub fn encode(&self, _: &mut [u8]) -> Result<usize, Error> { Err(Error) } pub fn decode(_: &[u8]) -> Result<Request<'_>, Error> { Err(Error) } }
impl Reply<'_> { pub fn encode(&self, _: &mut [u8]) -> Result<usize, Error> { Err(Error) } pub fn decode(_: &[u8]) -> Result<Reply<'_>, Error> { Err(Error) } }
