//! Common archive format as documented in Solaris ar(3HEAD).
//!
//! Member names and data borrow the input. The directory uses big-endian
//! integers even when the objects inside the archive use little-endian.

use crate::Error;

const MAGIC: &[u8; 8] = b"!<arch>\n";
const HEADER: usize = 60;
const NAME_WIDTH: usize = 16;
const DATE_WIDTH: usize = 12;
const SIZE_OFFSET: usize = 48;
const SIZE_WIDTH: usize = 10;
const TRAILER: &[u8; 2] = b"`\n";

#[derive(Debug)]
pub struct Member<'a> {
    pub header_offset: usize,
    pub data_offset: usize,
    pub name: &'a [u8],
    pub bytes: &'a [u8],
    date: &'a [u8],
}

impl Member<'_> {
    /// Solaris ar(3HEAD) stores the member's modification date in `ar_date`.
    pub fn timestamp(&self) -> Result<i64, Error> {
        std::str::from_utf8(self.date)
            .ok()
            .and_then(|date| date.trim_end_matches(' ').parse().ok())
            .ok_or(Error::new(
                self.header_offset + NAME_WIDTH,
                "invalid archive member timestamp",
            ))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct IndexEntry<'a> {
    pub name: &'a [u8],
    /// Index into the archive's ordinary members, in file order.
    pub member: usize,
}

#[derive(Debug)]
pub struct Archive<'a> {
    pub members: Vec<Member<'a>>,
    /// An absent directory differs from a present directory with no entries.
    pub index: Option<Vec<IndexEntry<'a>>>,
}

pub fn has_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

impl<'a> Archive<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let Scan { members, directory } = scan(bytes)?;
        let index = directory
            .map(|directory| directory.read(&members))
            .transpose()?;
        Ok(Self { members, index })
    }

    /// Read members when rebuilding a directory. Its contents are discarded,
    /// but its header, size and padding still have to frame the file.
    pub fn members_for_reindex(bytes: &'a [u8]) -> Result<Vec<Member<'a>>, Error> {
        Ok(scan(bytes)?.members)
    }
}

fn decimal(field: &[u8]) -> Option<usize> {
    let digits = field.split(|&byte| byte == b' ').next()?;
    if digits.is_empty() || field[digits.len()..].iter().any(|&byte| byte != b' ') {
        return None;
    }
    digits.iter().try_fold(0_usize, |value, &byte| {
        if !byte.is_ascii_digit() {
            return None;
        }
        value.checked_mul(10)?.checked_add(usize::from(byte - b'0'))
    })
}

fn read_long_names(mut bytes: &[u8], offset: usize) -> Result<Vec<(usize, &[u8])>, Error> {
    let mut names = Vec::new();
    let mut position = 0;
    // Some producers include the final alignment newline in ar_size.
    while !bytes.is_empty() && bytes != b"\n" {
        let len = bytes
            .windows(2)
            .position(|pair| pair == b"/\n")
            .ok_or(Error::new(
                offset + position,
                "unterminated archive long name",
            ))?;
        let name = &bytes[..len];
        if name.is_empty() || name.contains(&0) {
            return Err(Error::new(offset + position, "invalid archive long name"));
        }
        names.push((position, name));
        position += len + 2;
        bytes = &bytes[len + 2..];
    }
    Ok(names)
}

fn name_at<'a>(field: &'a [u8], names: &[(usize, &'a [u8])]) -> Option<&'a [u8]> {
    if let Some(digits) = field.strip_prefix(b"/") {
        let offset = decimal(digits)?;
        let entry = names
            .binary_search_by_key(&offset, |&(start, _)| start)
            .ok()?;
        return Some(names[entry].1);
    }
    let name = field.strip_suffix(b"/")?;
    (!name.is_empty() && !name.contains(&0)).then_some(name)
}

struct Scan<'a> {
    members: Vec<Member<'a>>,
    directory: Option<Directory<'a>>,
}

fn scan(bytes: &[u8]) -> Result<Scan<'_>, Error> {
    if !has_magic(bytes) {
        return Err(Error::new(0, "expected a common archive"));
    }
    let mut members = Vec::new();
    let mut directory = None;
    let mut names = None;
    let mut pos = MAGIC.len();
    while pos < bytes.len() {
        let header = bytes[pos..]
            .first_chunk::<HEADER>()
            .ok_or(Error::new(pos, "truncated archive member header"))?;
        if &header[HEADER - TRAILER.len()..] != TRAILER {
            return Err(Error::new(
                pos + HEADER - TRAILER.len(),
                "invalid archive member trailer",
            ));
        }
        let size = decimal(&header[SIZE_OFFSET..SIZE_OFFSET + SIZE_WIDTH])
            .ok_or(Error::new(pos + SIZE_OFFSET, "invalid archive member size"))?;
        let payload_start = pos + HEADER;
        let payload = bytes[payload_start..].get(..size).ok_or(Error::new(
            pos + SIZE_OFFSET,
            "truncated archive member data",
        ))?;
        let mut next = payload_start + size;
        if size % 2 != 0 {
            if bytes.get(next) != Some(&b'\n') {
                return Err(Error::new(next, "missing archive member padding"));
            }
            next += 1;
        }

        let raw_name = &header[..NAME_WIDTH];
        let len = raw_name
            .iter()
            .rposition(|&byte| byte != b' ')
            .map_or(0, |i| i + 1);
        let field = &raw_name[..len];
        match field {
            b"/" | b"/SYM64/" => {
                if pos != MAGIC.len() {
                    return Err(Error::new(pos, "archive symbol index must be first"));
                }
                let width = if field == b"/" { 4 } else { 8 };
                directory = Some(Directory {
                    bytes: payload,
                    offset: payload_start,
                    width,
                });
            }
            b"//" => {
                if names.is_some() || !members.is_empty() {
                    return Err(Error::new(pos, "misplaced archive long-name table"));
                }
                names = Some(read_long_names(payload, payload_start)?);
            }
            _ => {
                let name = name_at(field, names.as_deref().unwrap_or_default()).ok_or(
                    Error::new(pos, "invalid archive member name or long-name offset"),
                )?;
                members.push(Member {
                    header_offset: pos,
                    data_offset: payload_start,
                    name,
                    bytes: payload,
                    date: &header[NAME_WIDTH..NAME_WIDTH + DATE_WIDTH],
                });
            }
        }
        pos = next;
    }
    Ok(Scan { members, directory })
}

struct Directory<'a> {
    bytes: &'a [u8],
    offset: usize,
    width: usize,
}

impl<'a> Directory<'a> {
    fn read(&self, members: &[Member<'_>]) -> Result<Vec<IndexEntry<'a>>, Error> {
        let (count, rest) = self
            .bytes
            .split_at_checked(self.width)
            .ok_or(Error::new(self.offset, "truncated archive symbol index"))?;
        let offsets_size = usize::try_from(big_endian(count))
            .ok()
            .and_then(|count| count.checked_mul(self.width))
            .ok_or(Error::new(
                self.offset,
                "archive symbol index count exceeds bounds",
            ))?;
        let (offsets, mut names) = rest.split_at_checked(offsets_size).ok_or(Error::new(
            self.offset,
            "archive symbol index offsets exceed bounds",
        ))?;
        let count = offsets.len() / self.width;
        let mut name_offset = self.offset + self.width + offsets.len();
        if count > names.len() / 2 {
            return Err(Error::new(
                name_offset,
                "archive symbol index names exceed bounds",
            ));
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count)
            .map_err(|_| Error::new(self.offset, "cannot allocate archive symbol index"))?;
        for (index, word) in offsets.chunks_exact(self.width).enumerate() {
            let member_offset = big_endian(word);
            let member = members
                .binary_search_by_key(&member_offset, |member| member.header_offset as u64)
                .map_err(|_| {
                    Error::new(
                        self.offset + (index + 1) * self.width,
                        "archive symbol index offset does not name a member header",
                    )
                })?;
            let end = names
                .iter()
                .position(|&byte| byte == 0)
                .filter(|&end| end != 0)
                .ok_or(Error::new(
                    name_offset,
                    "invalid name in archive symbol index",
                ))?;
            entries.push(IndexEntry {
                name: &names[..end],
                member,
            });
            names = &names[end + 1..];
            name_offset += end + 1;
        }
        // Some producers count the optional alignment NUL in the index size.
        if !names.is_empty() && names != b"\0" {
            return Err(Error::new(
                name_offset,
                "unexpected bytes after archive index names",
            ));
        }
        Ok(entries)
    }
}

fn big_endian(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |value, &byte| (value << 8) | u64::from(byte))
}

fn member_end(offset: usize, size: usize) -> Result<usize, Error> {
    offset
        .checked_add(HEADER)
        .and_then(|end| end.checked_add(size))
        .and_then(|end| end.checked_add(size % 2))
        .filter(|&end| u32::try_from(end).is_ok())
        .ok_or(Error::new(offset, "archive exceeds the 32-bit index limit"))
}

fn write_header(output: &mut Vec<u8>, name: &[u8], size: usize) {
    output.extend_from_slice(name);
    output.resize(output.len() + NAME_WIDTH - name.len(), b' ');
    output
        .extend_from_slice(format!("{:<12}{:<6}{:<6}{:<8o}{size:<10}", 0, 0, 0, 0o644).as_bytes());
    output.extend_from_slice(TRAILER);
}

fn pad(output: &mut Vec<u8>, size: usize) {
    if size % 2 != 0 {
        output.push(b'\n');
    }
}

/// Emit a deterministic common archive with a 32-bit directory. The caller
/// supplies its member and symbol order and selects which symbols to index.
pub fn write(members: &[(&[u8], &[u8])], index: &[IndexEntry<'_>]) -> Result<Vec<u8>, Error> {
    let index_size = index.iter().try_fold(4, |size: usize, entry| {
        if entry.member >= members.len() || entry.name.is_empty() || entry.name.contains(&0) {
            return Err(Error::new(
                MAGIC.len(),
                "invalid archive output index entry",
            ));
        }
        size.checked_add(5)
            .and_then(|size| size.checked_add(entry.name.len()))
            .ok_or(Error::new(MAGIC.len(), "archive index size overflows"))
    })?;
    let mut end = member_end(MAGIC.len(), index_size)?;
    let mut long_names = Vec::new();
    let mut field_names = Vec::with_capacity(members.len());
    for &(name, _) in members {
        if name.is_empty()
            || name.starts_with(b"/")
            || name.contains(&0)
            || name.windows(2).any(|pair| pair == b"/\n")
        {
            return Err(Error::new(0, "invalid archive output member name"));
        }
        if name.len() >= NAME_WIDTH || name.contains(&b' ') {
            field_names.push(format!("/{}", long_names.len()).into_bytes());
            // Bound the long-name offset and payload before extending the table.
            let names_size = long_names
                .len()
                .checked_add(name.len())
                .and_then(|size| size.checked_add(2))
                .ok_or(Error::new(end, "archive long-name table size overflows"))?;
            member_end(end, names_size)?;
            long_names.extend_from_slice(name);
            long_names.extend_from_slice(b"/\n");
        } else {
            let mut field = name.to_vec();
            field.push(b'/');
            field_names.push(field);
        }
    }
    if !long_names.is_empty() {
        end = member_end(end, long_names.len())?;
    }
    let mut offsets = Vec::with_capacity(members.len());
    for &(_, bytes) in members {
        offsets.push(end as u32);
        end = member_end(end, bytes.len())?;
    }

    let mut output = Vec::new();
    output
        .try_reserve_exact(end)
        .map_err(|_| Error::new(0, "cannot allocate archive output"))?;
    output.extend_from_slice(MAGIC);
    write_header(&mut output, b"/", index_size);
    output.extend_from_slice(&(index.len() as u32).to_be_bytes());
    for entry in index {
        output.extend_from_slice(&offsets[entry.member].to_be_bytes());
    }
    for entry in index {
        output.extend_from_slice(entry.name);
        output.push(0);
    }
    pad(&mut output, index_size);
    if !long_names.is_empty() {
        write_header(&mut output, b"//", long_names.len());
        output.extend_from_slice(&long_names);
        pad(&mut output, long_names.len());
    }
    for (field, &(_, bytes)) in field_names.iter().zip(members) {
        write_header(&mut output, field, bytes.len());
        output.extend_from_slice(bytes);
        pad(&mut output, bytes.len());
    }
    Ok(output)
}
