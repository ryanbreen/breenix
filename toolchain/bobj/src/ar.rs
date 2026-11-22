use crate::{Error, UNIMPLEMENTED};

#[derive(Debug)]
pub struct Member<'a> {
    pub header_offset: usize,
    pub name: &'a [u8],
    pub bytes: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub struct IndexEntry<'a> {
    pub name: &'a [u8],
    pub member: usize,
}

#[derive(Debug)]
pub struct Archive<'a> {
    pub members: Vec<Member<'a>>,
    pub index: Option<Vec<IndexEntry<'a>>>,
}

impl<'a> Archive<'a> {
    pub fn parse(_: &'a [u8]) -> Result<Self, Error> {
        Err(UNIMPLEMENTED)
    }

    pub fn members_for_reindex(_: &'a [u8]) -> Result<Vec<Member<'a>>, Error> {
        Err(UNIMPLEMENTED)
    }
}

pub fn write(_: &[(&[u8], &[u8])], _: &[IndexEntry<'_>]) -> Result<Vec<u8>, Error> {
    Err(UNIMPLEMENTED)
}
