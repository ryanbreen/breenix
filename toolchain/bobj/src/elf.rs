use crate::{Error, UNIMPLEMENTED};

#[derive(Debug)]
pub struct Elf;

impl Elf {
    pub fn parse(_: &[u8]) -> Result<Self, Error> {
        Err(UNIMPLEMENTED)
    }
    pub fn section_name(&self, _: usize) -> Result<&[u8], Error> {
        Err(UNIMPLEMENTED)
    }
    pub fn symbols(&self, _: usize) -> Result<Symbols, Error> {
        Err(UNIMPLEMENTED)
    }
    pub fn relocations(&self, _: usize) -> Result<Relocations, Error> {
        Err(UNIMPLEMENTED)
    }
    pub fn program(&self, _: usize) -> Result<ProgramHeader, Error> {
        Err(UNIMPLEMENTED)
    }
}

#[derive(Debug)]
pub struct StringTable;

impl StringTable {
    pub fn parse(_: &[u8], _: usize) -> Result<Self, Error> {
        Err(UNIMPLEMENTED)
    }
    pub fn get(&self, _: u32) -> Result<&[u8], Error> {
        Err(UNIMPLEMENTED)
    }
}

#[derive(Debug)]
pub struct Symbols;

impl Symbols {
    pub fn get(&self, _: usize) -> Result<Symbol<'_>, Error> {
        Err(UNIMPLEMENTED)
    }
}

#[derive(Debug)]
pub struct Symbol<'a> {
    pub name: &'a [u8],
    pub binding: u8,
}

#[derive(Debug)]
pub struct Relocations;

impl Relocations {
    pub fn get(&self, _: usize) -> Result<Rela, Error> {
        Err(UNIMPLEMENTED)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Rela {
    pub offset: u64,
    pub kind: u32,
    pub symbol: u32,
    pub addend: i64,
}

#[derive(Debug)]
pub struct ProgramHeader {
    pub virtual_address: u64,
    pub file_size: u64,
    pub memory_size: u64,
}

pub struct RelSection<'a> {
    pub name: &'a [u8],
    pub kind: u32,
    pub flags: u64,
    pub alignment: u64,
    pub bytes: &'a [u8],
    pub size: u64,
    pub relocations: &'a [Rela],
}

pub struct RelSymbol<'a> {
    pub name: &'a [u8],
    pub section: u16,
    pub kind: u8,
    pub binding: u8,
    pub value: u64,
    pub size: u64,
}

pub fn write_rel(_: u16, _: &[RelSection<'_>], _: &[RelSymbol<'_>]) -> Result<Vec<u8>, Error> {
    Err(UNIMPLEMENTED)
}
