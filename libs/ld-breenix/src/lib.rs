#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

// API scaffold for the check-first commit.
use core::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    MissingDynamic,
    Bounds,
    UnsupportedRelocation(u32),
    UndefinedSymbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error<'a> {
    pub object: &'a [u8],
    pub offset: u64,
    pub kind: ErrorKind,
    pub detail: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct Object<'a> {
    name: &'a [u8],
}

impl<'a> Object<'a> {
    pub fn parse(name: &'a [u8], _file: &'a [u8], _bias: u64) -> Result<Self, Error<'a>> {
        Err(Error {
            object: name,
            offset: 0,
            kind: ErrorKind::MissingDynamic,
            detail: b"",
        })
    }

    pub fn name(self) -> &'a [u8] {
        self.name
    }

    pub fn relocate(self, _scope: &[Self], _write: impl FnMut(Patch)) -> Result<(), Error<'a>> {
        Ok(())
    }

    pub fn relro(self) -> Option<Range<u64>> {
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Patch {
    pub address: u64,
    pub value: u64,
}

#[derive(Clone, Copy, Default)]
pub struct Dependency {
    _from: usize,
    _to: usize,
}

#[derive(Clone, Copy, Default)]
pub struct InitState {
    _next: usize,
    _parent: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitKind {
    Preinit,
    Init,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InitArray {
    pub object: usize,
    pub kind: InitKind,
    pub range: Range<u64>,
}

pub struct Scope<'a, 's> {
    objects: &'s [Object<'a>],
}

impl<'a, 's> Scope<'a, 's> {
    pub fn load(
        root: Object<'a>,
        objects: &'s mut [Object<'a>],
        _edges: &'s mut [Dependency],
        _path: &mut [u8],
        _open: impl FnMut(&[u8]) -> Result<Option<Object<'a>>, Error<'a>>,
    ) -> Result<Self, Error<'a>> {
        objects.fill(root);
        Ok(Self { objects })
    }

    pub fn objects(&self) -> &[Object<'a>] {
        self.objects
    }

    pub fn initializers(
        &self,
        _state: &mut [InitState],
        _emit: impl FnMut(InitArray),
    ) -> Result<(), Error<'a>> {
        Ok(())
    }
}
