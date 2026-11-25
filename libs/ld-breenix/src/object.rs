use core::ops::Range;

use crate::elf::{Bytes, Image, PF_R, PF_W, PT_DYNAMIC, PT_GNU_RELRO, PT_TLS};
use crate::tls_module::TlsModule;
use crate::{Error, ErrorKind};

const DT_NEEDED: u64 = 1;
const DT_PLTRELSZ: u64 = 2;

const DT_SONAME: u64 = 14;
const DT_HASH: u64 = 4;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;
const DT_STRSZ: u64 = 10;
const DT_SYMENT: u64 = 11;
const DT_INIT: u64 = 12;
const DT_RPATH: u64 = 15;
const DT_SYMBOLIC: u64 = 16;
const DT_REL: u64 = 17;
const DT_PLTREL: u64 = 20;
const DT_TEXTREL: u64 = 22;
const DT_JMPREL: u64 = 23;
const DT_INIT_ARRAY: u64 = 25;
const DT_INIT_ARRAYSZ: u64 = 27;
const DT_RUNPATH: u64 = 29;
const DT_FLAGS: u64 = 30;
const DT_PREINIT_ARRAY: u64 = 32;
const DT_PREINIT_ARRAYSZ: u64 = 33;
const DT_RELR: u64 = 36;
const DT_VERSYM: u64 = 0x6fff_fff0;
const DYN_SIZE: u64 = 16;
const SYMBOL_SIZE: u64 = 24;
const RELA_SIZE: u64 = 24;

const SHN_UNDEF: u16 = 0;
const SHN_ABS: u16 = 0xfff1;
const STB_LOCAL: u8 = 0;
const STB_GLOBAL: u8 = 1;
const STB_WEAK: u8 = 2;
const STV_DEFAULT: u8 = 0;
const STV_PROTECTED: u8 = 3;

const R_X86_64_64: u32 = 1;
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;
const R_X86_64_RELATIVE: u32 = 8;
const R_X86_64_DTPMOD64: u32 = 16;

#[derive(Debug, Clone, Copy)]
pub struct Object<'a> {
    image: Image<'a>,
    dynamic: Bytes<'a>,
    strings: Bytes<'a>,
    symbols: Bytes<'a>,
    hash: Bytes<'a>,
    buckets: u64,
    chains: u64,
    rela: Option<Bytes<'a>>,
    plt: Option<Bytes<'a>>,
    #[cfg(feature = "interpreter")]
    soname: Option<LinkString<'a>>,
    pub(crate) runpath: Option<LinkString<'a>>,
    pub(crate) preinit: Option<Span>,
    pub(crate) init: Option<Span>,
    relro: Option<Span>,
    tls: Option<TlsModule>,
}

/// An eight-byte little-endian store to a validated writable load span.
#[derive(Debug, PartialEq, Eq)]
pub struct Patch {
    pub address: u64,
    pub value: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LinkString<'a> {
    pub bytes: &'a [u8],
    pub offset: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Span {
    start: u64,
    size: u64,
}

impl Span {
    pub fn range(self) -> Range<u64> {
        self.start..self.start + self.size
    }
}

#[derive(Clone, Copy)]
struct Entry {
    value: u64,
    offset: u64,
}

struct Tags<'a> {
    entries: [Option<Entry>; DT_RELR as usize + 1],
    dynamic: Bytes<'a>,
}

impl<'a> Tags<'a> {
    fn parse(dynamic: Bytes<'a>) -> Result<Self, Error<'a>> {
        if dynamic.data.len() as u64 % DYN_SIZE != 0 {
            return Err(dynamic.error(ErrorKind::InvalidDynamic));
        }
        let mut entries = [None; DT_RELR as usize + 1];
        for offset in (0..dynamic.data.len() as u64).step_by(DYN_SIZE as usize) {
            let kind = dynamic.u64(offset)?;
            let entry = Entry {
                value: dynamic.u64(offset + 8)?,
                offset: dynamic.offset + offset,
            };
            if kind == 0 {
                return Ok(Self {
                    entries,
                    dynamic: dynamic.slice(0, offset)?,
                });
            }
            // These tags require semantics outside the KD u3 input profile.
            if matches!(
                kind,
                DT_REL | DT_RELR | DT_VERSYM | DT_SYMBOLIC | DT_TEXTREL | DT_INIT
            ) || (kind == DT_FLAGS && entry.value & 6 != 0)
            {
                return Err(Error {
                    offset: entry.offset,
                    ..dynamic.error(ErrorKind::UnsupportedDynamic(kind))
                });
            }
            if matches!(
                kind,
                DT_PLTRELSZ
                    | DT_SONAME
                    | DT_HASH
                    | DT_STRTAB
                    | DT_SYMTAB
                    | DT_RELA
                    | DT_RELASZ
                    | DT_RELAENT
                    | DT_STRSZ
                    | DT_SYMENT
                    | DT_RPATH
                    | DT_PLTREL
                    | DT_JMPREL
                    | DT_INIT_ARRAY
                    | DT_INIT_ARRAYSZ
                    | DT_RUNPATH
                    | DT_FLAGS
                    | DT_PREINIT_ARRAY
                    | DT_PREINIT_ARRAYSZ
            ) && entries[kind as usize].replace(entry).is_some()
            {
                return Err(Error {
                    offset: entry.offset,
                    ..dynamic.error(ErrorKind::InvalidDynamic)
                });
            }
        }
        Err(dynamic.error(ErrorKind::InvalidDynamic))
    }

    fn get(&self, kind: u64) -> Option<Entry> {
        self.entries[kind as usize]
    }

    fn require(&self, kind: u64) -> Result<Entry, Error<'a>> {
        self.get(kind)
            .ok_or_else(|| self.dynamic.error(ErrorKind::InvalidDynamic))
    }

    fn expect_value(&self, kind: u64, value: u64) -> Result<(), Error<'a>> {
        let entry = self.require(kind)?;
        if entry.value == value {
            return Ok(());
        }
        Err(Error {
            offset: entry.offset,
            ..self.dynamic.error(ErrorKind::InvalidDynamic)
        })
    }

    fn pair(&self, address: u64, size: u64, unit: u64) -> Result<Option<(Entry, u64)>, Error<'a>> {
        match (self.get(address), self.get(size)) {
            (None, None) => Ok(None),
            (Some(address), Some(size)) if size.value % unit == 0 => {
                Ok(Some((address, size.value)))
            }
            (Some(entry), _) | (_, Some(entry)) => Err(Error {
                offset: entry.offset,
                ..self.dynamic.error(ErrorKind::InvalidDynamic)
            }),
        }
    }

    fn array(&self, image: Image<'a>, address: u64, size: u64) -> Result<Option<Span>, Error<'a>> {
        let Some((address, size)) = self.pair(address, size, 8)? else {
            return Ok(None);
        };
        if size == 0 {
            return Ok(None);
        }
        let start = image.address(address.value, size, PF_R, address.offset)?;
        Ok(Some(Span { start, size }))
    }
}

#[derive(Clone, Copy)]
struct Symbol<'a> {
    name: &'a [u8],
    offset: u64,
    binding: u8,
    visibility: u8,
    kind: u8,
    section: u16,
    value: u64,
}

impl<'a> Object<'a> {
    /// Read an AMD64 ELF file through its program headers. `name` is the
    /// caller's canonical absolute pathname for dependency search and errors.
    pub fn parse(name: &'a [u8], file: &'a [u8], bias: u64) -> Result<Self, Error<'a>> {
        let image = Image::parse(
            Bytes {
                name,
                data: file,
                offset: 0,
            },
            bias,
        )?;
        let mut dynamic = None;
        let mut relro = None;
        let mut tls = None;
        for segment in image.segments() {
            let segment = segment?;
            match segment.kind {
                PT_TLS => {
                    let error = || image.at(segment.header_offset, ErrorKind::InvalidProgramHeader);
                    let align = segment.align.max(1);
                    if tls.is_some()
                        || segment.file_size > segment.mem_size
                        || !align.is_power_of_two()
                        || segment.address % align != segment.offset % align
                    {
                        return Err(error());
                    }
                    let mut template = TlsModule {
                        image: core::ptr::null(),
                        file_size: segment.file_size.try_into().map_err(|_| error())?,
                        mem_size: segment.mem_size.try_into().map_err(|_| error())?,
                        align: align.try_into().map_err(|_| error())?,
                    };
                    if template.file_size != 0 {
                        let bytes = image.file_at(
                            segment.address,
                            segment.file_size,
                            segment.header_offset,
                        )?;
                        if bytes.offset != segment.offset {
                            return Err(error());
                        }
                        // file_at validated the range in a readable PT_LOAD.
                        template.image = (image.bias + segment.address) as *const u8;
                    }
                    tls = Some(template);
                }
                PT_DYNAMIC => {
                    let bytes =
                        image.file_at(segment.address, segment.file_size, segment.header_offset)?;
                    if bytes.offset != segment.offset
                        || segment.file_size > segment.mem_size
                        || dynamic.replace(bytes).is_some()
                    {
                        return Err(
                            image.at(segment.header_offset, ErrorKind::InvalidProgramHeader)
                        );
                    }
                }
                PT_GNU_RELRO if segment.mem_size != 0 => {
                    let start = image.address(
                        segment.address,
                        segment.mem_size,
                        PF_W,
                        segment.header_offset,
                    )?;
                    if relro
                        .replace(Span {
                            start,
                            size: segment.mem_size,
                        })
                        .is_some()
                    {
                        return Err(
                            image.at(segment.header_offset, ErrorKind::InvalidProgramHeader)
                        );
                    }
                }
                _ => {}
            }
        }
        let dynamic = dynamic.ok_or_else(|| image.at(0, ErrorKind::MissingDynamic))?;
        let tags = Tags::parse(dynamic)?;
        let runpath = tags.get(DT_RUNPATH);
        if let Some(rpath) = tags.get(DT_RPATH).filter(|_| runpath.is_none()) {
            return Err(image.at(rpath.offset, ErrorKind::UnsupportedDynamic(DT_RPATH)));
        }
        let strtab = tags.require(DT_STRTAB)?;
        let strings = image.file_at(strtab.value, tags.require(DT_STRSZ)?.value, strtab.offset)?;
        let runpath = runpath.map(|entry| string(strings, entry)).transpose()?;
        #[cfg(feature = "interpreter")]
        let soname = tags
            .get(DT_SONAME)
            .map(|entry| string(strings, entry))
            .transpose()?;

        let hash_tag = tags.require(DT_HASH)?;
        let hash_header = image.file_at(hash_tag.value, 8, hash_tag.offset)?;
        let buckets = u64::from(hash_header.u32(0)?);
        let chains = u64::from(hash_header.u32(4)?);
        if buckets == 0 || chains == 0 {
            return Err(hash_header.error(ErrorKind::InvalidHash));
        }
        // The counts are 32-bit ELF words. Their sum and byte sizes fit u64.
        let hash = image.file_at(hash_tag.value, 4 * (2 + buckets + chains), hash_tag.offset)?;
        tags.expect_value(DT_SYMENT, SYMBOL_SIZE)?;
        let symtab = tags.require(DT_SYMTAB)?;
        let symbols = image.file_at(symtab.value, SYMBOL_SIZE * chains, symtab.offset)?;

        let rela = if let Some((entry, size)) = tags.pair(DT_RELA, DT_RELASZ, RELA_SIZE)? {
            tags.expect_value(DT_RELAENT, RELA_SIZE)?;
            Some(image.file_at(entry.value, size, entry.offset)?)
        } else {
            None
        };
        let plt = if let Some((entry, size)) = tags.pair(DT_JMPREL, DT_PLTRELSZ, RELA_SIZE)? {
            tags.expect_value(DT_PLTREL, DT_RELA)?;
            Some(image.file_at(entry.value, size, entry.offset)?)
        } else {
            None
        };
        Ok(Self {
            image,
            dynamic: tags.dynamic,
            strings,
            symbols,
            hash,
            buckets,
            chains,
            rela,
            plt,
            #[cfg(feature = "interpreter")]
            soname,
            runpath,
            preinit: tags.array(image, DT_PREINIT_ARRAY, DT_PREINIT_ARRAYSZ)?,
            init: tags.array(image, DT_INIT_ARRAY, DT_INIT_ARRAYSZ)?,
            relro,
            tls,
        })
    }

    /// TLS template metadata in the mapped image, if the object has PT_TLS.
    pub fn tls_module(self) -> Option<TlsModule> {
        self.tls
    }

    pub fn name(self) -> &'a [u8] {
        self.image.file.name
    }

    pub fn bias(self) -> u64 {
        self.image.bias
    }

    pub fn relro(self) -> Option<Range<u64>> {
        self.relro.map(Span::range)
    }

    pub(crate) fn is_shared(self) -> bool {
        self.image.shared
    }

    pub(crate) fn at(self, offset: u64, kind: ErrorKind) -> Error<'a> {
        self.image.at(offset, kind)
    }

    pub(crate) fn needed(self) -> impl Iterator<Item = Result<LinkString<'a>, Error<'a>>> {
        (0..self.dynamic.data.len() as u64)
            .step_by(DYN_SIZE as usize)
            .filter_map(move |offset| match self.dynamic.u64(offset) {
                Ok(DT_NEEDED) => Some(self.dynamic.u64(offset + 8).and_then(|value| {
                    string(
                        self.strings,
                        Entry {
                            value,
                            offset: self.dynamic.offset + offset,
                        },
                    )
                })),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
    }

    fn symbol(self, index: u64) -> Result<Symbol<'a>, Error<'a>> {
        let bytes = self.symbols.slice(index * SYMBOL_SIZE, SYMBOL_SIZE)?;
        let name = string(
            self.strings,
            Entry {
                offset: bytes.offset,
                value: u64::from(bytes.u32(0)?),
            },
        )?;
        let info = bytes.u16(4)?;
        Ok(Symbol {
            name: name.bytes,
            offset: bytes.offset,
            binding: (info as u8) >> 4,
            visibility: ((info >> 8) as u8) & 3,
            kind: (info as u8) & 15,
            section: bytes.u16(6)?,
            value: bytes.u64(8)?,
        })
    }

    fn symbol_address(self, symbol: Symbol<'a>) -> Result<u64, Error<'a>> {
        // TLS offsets and indirect functions require other handling.
        if !matches!(symbol.kind, 0..=3) {
            return Err(self.at(symbol.offset, ErrorKind::InvalidSymbol));
        }
        match symbol.section {
            SHN_ABS => Ok(symbol.value),
            1..=0xfeff => self
                .bias()
                .checked_add(symbol.value)
                .ok_or_else(|| self.at(symbol.offset, ErrorKind::InvalidSymbol)),
            _ => Err(self.at(symbol.offset, ErrorKind::InvalidSymbol)),
        }
    }

    /// Find an externally visible definition in this object's SysV hash table.
    pub fn lookup(self, name: &[u8]) -> Result<Option<u64>, Error<'a>> {
        let bucket = u64::from(elf_hash(name)) % self.buckets;
        let mut link = 4 * (2 + bucket);
        // An index visited twice can't lead to an unseen definition.
        for _ in 0..self.chains {
            let index = u64::from(self.hash.u32(link)?);
            if index == 0 {
                return Ok(None);
            }
            if index >= self.chains {
                break;
            }
            let symbol = self.symbol(index)?;
            if symbol.name == name
                && symbol.section != SHN_UNDEF
                && matches!(symbol.binding, STB_GLOBAL | STB_WEAK)
                && matches!(symbol.visibility, STV_DEFAULT | STV_PROTECTED)
            {
                return self.symbol_address(symbol).map(Some);
            }
            link = 4 * (2 + self.buckets + index);
        }
        Err(self.at(self.hash.offset + link, ErrorKind::InvalidHash))
    }

    /// Search an already ordered scope, stopping at its first definition.
    pub fn lookup_scope(
        objects: impl IntoIterator<Item = Self>,
        name: &[u8],
    ) -> Result<Option<u64>, Error<'a>> {
        for object in objects {
            if let Some(address) = object.lookup(name)? {
                return Ok(Some(address));
            }
        }
        Ok(None)
    }

    #[cfg(feature = "interpreter")]
    pub(crate) fn interpreter_name(self) -> Option<&'a core::ffi::CStr> {
        self.image.interpreter_name()
    }

    #[cfg(feature = "interpreter")]
    pub(crate) fn matches_name(self, name: &[u8]) -> bool {
        self.name() == name
            || (!name.contains(&b'/')
                && (self.soname.is_some_and(|soname| soname.bytes == name)
                    || self.name().rsplit(|&byte| byte == b'/').next() == Some(name)))
    }

    #[cfg(feature = "interpreter")]
    pub(crate) fn info(self, module: usize) -> Result<crate::dl_abi::ObjectInfo, Error<'a>> {
        use crate::dl_abi::{LoadedObject, ObjectInfo};
        let (phdr, phnum) = self.image.program_headers();
        let dynamic = self
            .image
            .runtime_offset(self.dynamic.offset, DYN_SIZE)
            .ok_or_else(|| self.dynamic.error(ErrorKind::InvalidDynamic))?;
        let text_base = self.image.section_base(b".text").unwrap_or(0);
        let got_base = self.image.section_base(b".got").unwrap_or(0);
        Ok(ObjectInfo {
            object: LoadedObject {
                name: self.name().as_ptr().cast(),
                bias: self.bias(),
                phdr,
                phnum,
                dynamic: dynamic as *const _,
                tls_module_id: module,
            },
            text_base,
            got_base,
        })
    }

    #[cfg(feature = "interpreter")]
    pub(crate) fn address_info(self, address: u64) -> Option<crate::dl_abi::DlInfo> {
        let offset = address.checked_sub(self.bias())?;
        self.image.address(offset, 1, 0, 0).ok()?;
        let base = self
            .image
            .segments()
            .filter_map(Result::ok)
            .find(|segment| segment.kind == crate::elf::PT_LOAD)?
            .address
            + self.bias();
        let nearest = (1..self.chains)
            .filter_map(|index| {
                let symbol = self.symbol(index).ok()?;
                let value = self.symbol_address(symbol).ok()?;
                (value >= base
                    && value <= address
                    && !symbol.name.is_empty()
                    && symbol.section != SHN_ABS)
                    .then_some((symbol.name, value))
            })
            .max_by_key(|&(_, value)| value);
        Some(crate::dl_abi::DlInfo {
            dli_fname: self.name().as_ptr().cast(),
            dli_fbase: base as *mut _,
            dli_sname: nearest.map_or(core::ptr::null(), |(name, _)| name.as_ptr().cast()),
            dli_saddr: nearest.map_or(core::ptr::null_mut(), |(_, value)| value as *mut _),
        })
    }

    fn resolve(self, scope: &[Self], index: u64, offset: u64) -> Result<u64, Error<'a>> {
        if index == 0 {
            return Ok(0);
        }
        if index >= self.chains {
            return Err(self.at(offset, ErrorKind::InvalidRelocation));
        }
        let symbol = self.symbol(index)?;
        if !matches!(symbol.binding, STB_LOCAL | STB_GLOBAL | STB_WEAK)
            || !matches!(symbol.kind, 0..=3)
        {
            return Err(self.at(symbol.offset, ErrorKind::InvalidSymbol));
        }
        if symbol.binding == STB_LOCAL || symbol.visibility != STV_DEFAULT {
            if symbol.section != SHN_UNDEF {
                return self.symbol_address(symbol);
            }
        } else if let Some(address) = Self::lookup_scope(scope.iter().copied(), symbol.name)? {
            return Ok(address);
        }
        if symbol.section == SHN_UNDEF && symbol.binding == STB_WEAK {
            return Ok(0);
        }
        Err(Error {
            detail: symbol.name,
            ..self.at(offset, ErrorKind::UndefinedSymbol)
        })
    }

    /// Calculate stores from immutable file bytes for eager relocation.
    /// Discard the pending load on error. Stores emitted before the error
    /// may already have been applied to the pending image by the caller.
    pub fn relocate(self, scope: &[Self], write: impl FnMut(Patch)) -> Result<(), Error<'a>> {
        let module = scope
            .iter()
            .filter(|object| object.tls.is_some())
            .position(|object| object.name() == self.name() && object.bias() == self.bias())
            .map_or(0, |slot| slot + 1);
        self.relocate_with_module(scope, module, write)
    }

    pub(crate) fn relocate_with_module(
        self,
        scope: &[Self],
        module: usize,
        mut write: impl FnMut(Patch),
    ) -> Result<(), Error<'a>> {
        for table in [self.rela, self.plt].into_iter().flatten() {
            for pos in (0..table.data.len() as u64).step_by(RELA_SIZE as usize) {
                let entry = table.slice(pos, RELA_SIZE)?;
                let address = self.image.address(entry.u64(0)?, 8, PF_W, entry.offset)?;
                let info = entry.u64(8)?;
                let index = info >> 32;
                let addend = entry.u64(16)? as i64;
                let value = match info as u32 {
                    R_X86_64_RELATIVE if index == 0 => self
                        .bias()
                        .checked_add_signed(addend)
                        .ok_or_else(|| entry.error(ErrorKind::InvalidRelocation))?,
                    R_X86_64_RELATIVE => return Err(entry.error(ErrorKind::InvalidRelocation)),
                    // The result of S + A occupies an ELF word64, including signed addends.
                    R_X86_64_64 => self
                        .resolve(scope, index, entry.offset)?
                        .wrapping_add_signed(addend),
                    R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                        self.resolve(scope, index, entry.offset)?
                    }
                    // AMD64 psABI §4.4.1: the local-dynamic GOT pair names
                    // this object's module and has a 0 offset.
                    R_X86_64_DTPMOD64 if index == 0 && addend == 0 && module != 0 => module as u64,
                    R_X86_64_DTPMOD64 if index == 0 && addend == 0 => {
                        return Err(entry.error(ErrorKind::InvalidRelocation))
                    }
                    kind => return Err(entry.error(ErrorKind::UnsupportedRelocation(kind))),
                };
                write(Patch { address, value });
            }
        }
        Ok(())
    }
}

fn string<'a>(strings: Bytes<'a>, entry: Entry) -> Result<LinkString<'a>, Error<'a>> {
    let start = usize::try_from(entry.value).ok();
    let bytes = start
        .and_then(|start| strings.data.get(start..))
        .and_then(|tail| {
            let end = tail.iter().position(|&byte| byte == 0)?;
            tail.get(..end)
        })
        .ok_or_else(|| Error {
            offset: entry.offset,
            ..strings.error(ErrorKind::InvalidString)
        })?;
    Ok(LinkString {
        bytes,
        offset: entry.offset,
    })
}

fn elf_hash(name: &[u8]) -> u32 {
    // gABI §8.5 folds the high nibble after incorporating each byte.
    let mut hash = 0u32;
    for &byte in name {
        hash = (hash << 4).wrapping_add(u32::from(byte));
        let high = hash & 0xf000_0000;
        hash = (hash ^ (high >> 24)) & !high;
    }
    hash
}
