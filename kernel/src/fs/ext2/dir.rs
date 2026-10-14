use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;

/// Directory entry file types (from d_file_type field, ext2 feature)
pub const EXT2_FT_UNKNOWN: u8 = 0;
pub const EXT2_FT_REG_FILE: u8 = 1;
pub const EXT2_FT_DIR: u8 = 2;
pub const EXT2_FT_CHRDEV: u8 = 3;
pub const EXT2_FT_BLKDEV: u8 = 4;
pub const EXT2_FT_FIFO: u8 = 5;
pub const EXT2_FT_SOCK: u8 = 6;
pub const EXT2_FT_SYMLINK: u8 = 7;

/// ext2 directory entry structure (variable size)
/// Layout on disk:
/// - inode: u32 (4 bytes)
/// - rec_len: u16 (2 bytes) - total size of this entry
/// - name_len: u8 (1 byte) - actual name length
/// - file_type: u8 (1 byte) - file type (if feature enabled)
/// - name: [u8; name_len] - filename (not null-terminated)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext2DirEntryRaw {
    pub inode: u32,    // Inode number (0 = deleted entry)
    pub rec_len: u16,  // Total entry length (includes padding)
    pub name_len: u8,  // Name length
    pub file_type: u8, // File type (EXT2_FT_*)
}

/// Parsed directory entry with owned name
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub inode: u32,
    pub file_type: u8,
    pub name: String,
    /// `name` is the entry's on-disk bytes, not a lossy rendering of them.
    pub name_is_exact: bool,
}

impl DirEntry {
    /// Check if this is the "." entry
    pub fn is_dot(&self) -> bool {
        self.name == "."
    }

    /// Check if this is the ".." entry
    pub fn is_dotdot(&self) -> bool {
        self.name == ".."
    }

    /// Check if entry is a directory
    pub fn is_dir(&self) -> bool {
        self.file_type == EXT2_FT_DIR
    }

    /// Check if entry is a regular file
    pub fn is_file(&self) -> bool {
        self.file_type == EXT2_FT_REG_FILE
    }
}

/// Directory iterator/parser
pub struct DirReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> DirReader<'a> {
    /// Create a new directory reader from raw block data
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    /// Read a packed u32 from the data buffer at the given offset
    fn read_u32(&self, offset: usize) -> Option<u32> {
        if offset + 4 > self.data.len() {
            return None;
        }
        Some(u32::from_le_bytes([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ]))
    }

    /// Read a packed u16 from the data buffer at the given offset
    fn read_u16(&self, offset: usize) -> Option<u16> {
        if offset + 2 > self.data.len() {
            return None;
        }
        Some(u16::from_le_bytes([
            self.data[offset],
            self.data[offset + 1],
        ]))
    }
}

/// A live directory entry with where its record lies in the directory's data.
pub struct PositionedEntry {
    /// Byte offset of the entry's record within the directory's data.
    pub offset: usize,
    /// Byte offset of the record that follows it.
    pub next: usize,
    pub entry: DirEntry,
}

impl<'a> DirReader<'a> {
    /// Iterate live entries together with their record offsets. Records are
    /// found only by following the `rec_len` chain from the start of the
    /// data, so every offset yielded is a record boundary.
    pub fn positioned(self) -> impl Iterator<Item = PositionedEntry> + 'a {
        let mut reader = self;
        core::iter::from_fn(move || reader.next_positioned())
    }

    fn next_positioned(&mut self) -> Option<PositionedEntry> {
        // Loop to skip deleted entries
        loop {
            // Check if we're at or past the end of the data
            if self.offset >= self.data.len() {
                return None;
            }

            // Ensure we have at least enough bytes for the header
            let header_size = size_of::<Ext2DirEntryRaw>();
            if self.offset + header_size > self.data.len() {
                return None;
            }

            // Read the entry header fields manually (packed struct)
            let inode = self.read_u32(self.offset)?;
            let rec_len = self.read_u16(self.offset + 4)?;
            let name_len = self.data[self.offset + 6];
            let file_type = self.data[self.offset + 7];

            // Validate rec_len
            if rec_len == 0 || rec_len < header_size as u16 {
                // Invalid entry, stop iteration
                return None;
            }

            // Check if rec_len would exceed buffer
            if self.offset + rec_len as usize > self.data.len() {
                return None;
            }

            // Save the current offset for name extraction
            let record_offset = self.offset;
            let name_offset = self.offset + header_size;

            // Advance offset to next entry
            self.offset += rec_len as usize;

            // Skip deleted entries (inode == 0)
            if inode == 0 {
                continue;
            }

            // Validate name_len: the name lies inside its own record
            if name_len == 0 || header_size + name_len as usize > rec_len as usize {
                // Invalid name length, stop iteration
                return None;
            }

            // Extract name bytes
            let name_bytes = &self.data[name_offset..name_offset + name_len as usize];

            // Convert to String, replacing invalid UTF-8 with replacement char
            let name_is_exact = core::str::from_utf8(name_bytes).is_ok();
            let name = String::from_utf8_lossy(name_bytes).into_owned();

            return Some(PositionedEntry {
                offset: record_offset,
                next: self.offset,
                entry: DirEntry {
                    inode,
                    file_type,
                    name,
                    name_is_exact,
                },
            });
        }
    }
}

impl<'a> Iterator for DirReader<'a> {
    type Item = DirEntry;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_positioned().map(|positioned| positioned.entry)
    }
}

/// Parse all directory entries from a data buffer
pub fn parse_directory(data: &[u8]) -> Vec<DirEntry> {
    DirReader::new(data).collect()
}

/// Find a specific entry by name in directory data
///
/// A name that is not valid UTF-8 is matched by nothing: its lossy rendering
/// is not its name.
pub fn find_entry(data: &[u8], name: &str) -> Option<DirEntry> {
    DirReader::new(data).find(|e| e.name_is_exact && e.name == name)
}

/// Result of finding a directory entry with its offset
pub struct DirEntryLocation {
    /// The directory entry itself
    pub entry: DirEntry,
    /// Byte offset of this entry within the directory data
    pub offset: usize,
    /// The rec_len of this entry
    pub rec_len: u16,
    /// Byte offset of the previous entry (None if this is first)
    pub prev_offset: Option<usize>,
    /// The rec_len of the previous entry (if any)
    pub prev_rec_len: Option<u16>,
}

/// Find a directory entry by name and return its location info
///
/// This is useful for unlink operations where we need to know the exact
/// position of the entry to remove it.
pub fn find_entry_location(data: &[u8], name: &str) -> Option<DirEntryLocation> {
    let mut offset = 0usize;
    let mut prev_offset: Option<usize> = None;
    let mut prev_rec_len: Option<u16> = None;

    while offset < data.len() {
        // Ensure we have at least enough bytes for the header
        if offset + 8 > data.len() {
            return None;
        }

        // Read the entry header fields
        let inode = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        let rec_len = u16::from_le_bytes([data[offset + 4], data[offset + 5]]);
        let name_len = data[offset + 6];
        let file_type = data[offset + 7];

        // Validate rec_len
        if rec_len == 0 || rec_len < 8 {
            return None;
        }

        // Check if rec_len would exceed buffer
        if offset + rec_len as usize > data.len() {
            return None;
        }

        // Skip deleted entries (inode == 0)
        if inode != 0 {
            // Extract name
            let name_offset = offset + 8;
            if name_offset + name_len as usize <= data.len() {
                let entry_name = &data[name_offset..name_offset + name_len as usize];
                if let Ok(entry_name_str) = core::str::from_utf8(entry_name) {
                    if entry_name_str == name {
                        return Some(DirEntryLocation {
                            entry: DirEntry {
                                inode,
                                file_type,
                                name: String::from(entry_name_str),
                                name_is_exact: true,
                            },
                            offset,
                            rec_len,
                            prev_offset,
                            prev_rec_len,
                        });
                    }
                }
            }
        }

        // Move to next entry
        prev_offset = Some(offset);
        prev_rec_len = Some(rec_len);
        offset += rec_len as usize;
    }

    None
}

/// Remove a directory entry by setting its inode to 0 and merging with previous entry if possible
///
/// Returns the inode number of the removed entry on success.
///
/// # Arguments
/// * `data` - Mutable directory data buffer
/// * `name` - Name of the entry to remove
/// * `block_size` - The filesystem block size; no entry spans two blocks
///
/// # Returns
/// * `Ok(u32)` - Inode number of the removed entry
/// * `Err(&'static str)` - Error message
pub fn remove_entry(data: &mut [u8], name: &str, block_size: usize) -> Result<u32, &'static str> {
    // Cannot remove . or ..
    if name == "." || name == ".." {
        return Err("Cannot remove . or ..");
    }

    // Find the entry location
    let location = raw_location(data, name).ok_or("Entry not found")?;
    let removed_inode = location.inode;

    // If there's a previous entry in the same block, extend its rec_len to
    // include this entry (effectively "absorbing" this entry's space). The
    // first entry of a block has its predecessor in another block, which a
    // record may not reach into.
    let same_block = |prev_off: usize| prev_off / block_size == location.offset / block_size;
    if let (Some(prev_off), Some(prev_rec)) = (
        location.prev_offset.filter(|prev_off| same_block(*prev_off)),
        location.prev_rec_len,
    ) {
        // Calculate new rec_len for previous entry
        let new_prev_rec_len = prev_rec + location.rec_len;

        // Update the previous entry's rec_len field
        data[prev_off + 4] = (new_prev_rec_len & 0xFF) as u8;
        data[prev_off + 5] = ((new_prev_rec_len >> 8) & 0xFF) as u8;
    } else {
        // This is the first entry - just set inode to 0
        data[location.offset] = 0;
        data[location.offset + 1] = 0;
        data[location.offset + 2] = 0;
        data[location.offset + 3] = 0;
    }

    Ok(removed_inode)
}

/// Minimum directory entry size (inode + rec_len + name_len + file_type)
const MIN_DIR_ENTRY_SIZE: usize = 8;

/// Calculate the required size for a directory entry with the given name
///
/// Directory entries must be 4-byte aligned.
fn required_entry_size(name_len: usize) -> usize {
    // Header (8 bytes) + name + padding to 4-byte boundary
    let raw_size = MIN_DIR_ENTRY_SIZE + name_len;
    (raw_size + 3) & !3
}

/// Add a new directory entry to a directory's data
///
/// This function finds space in the existing directory data for a new entry,
/// either by using a deleted entry's space or by splitting an existing entry's
/// padding space.
///
/// # Arguments
/// * `dir_data` - Mutable directory data buffer
/// * `new_inode` - Inode number for the new entry
/// * `name` - Name for the new entry
/// * `file_type` - File type (EXT2_FT_* constant)
///
/// # Returns
/// * `Ok(offset)` - Byte offset of the added entry
/// * `Err(msg)` - No space available or other error
pub fn add_directory_entry(
    dir_data: &mut Vec<u8>,
    new_inode: u32,
    name: &str,
    file_type: u8,
) -> Result<usize, &'static str> {
    add_directory_entry_bytes(dir_data, new_inode, name.as_bytes(), file_type)
}

pub fn add_directory_entry_bytes(dir_data: &mut Vec<u8>, new_inode: u32, name_bytes: &[u8], file_type: u8) -> Result<usize, &'static str> {
    if name_bytes.is_empty() || name_bytes.len() > 255 {
        return Err("Invalid name length");
    }
    match find_room(dir_data, name_bytes.len())? {
        Some(Room::Empty { offset, rec_len }) => {
            // An empty record big enough: reuse it whole.
            write_dir_entry(dir_data, offset, new_inode, rec_len as u16, name_bytes, file_type);
            Ok(offset)
        }
        Some(Room::Tail { offset, rec_len, used }) => {
            // A live record's unused tail: shrink the record and use the rest.
            dir_data[offset + 4..offset + 6].copy_from_slice(&(used as u16).to_le_bytes());
            let new_offset = offset + used;
            write_dir_entry(dir_data, new_offset, new_inode, (rec_len - used) as u16, name_bytes, file_type);
            Ok(new_offset)
        }
        None => Err("No space in directory"),
    }
}

/// Does some record of `dir_data` have room for an entry with a `name_len`-byte name?
/// When none does, the directory needs another block before the entry can be added.
pub fn has_room(dir_data: &[u8], name_len: usize) -> Result<bool, &'static str> {
    Ok(find_room(dir_data, name_len)?.is_some())
}

enum Room {
    Empty { offset: usize, rec_len: usize },
    Tail { offset: usize, rec_len: usize, used: usize },
}

/// The first record with room for a `name_len`-byte name: an empty record at least that big,
/// or a live record whose unused tail is.
fn find_room(dir_data: &[u8], name_len: usize) -> Result<Option<Room>, &'static str> {
    let new_entry_size = required_entry_size(name_len);
    let mut offset = 0usize;
    while offset + MIN_DIR_ENTRY_SIZE <= dir_data.len() {
        let entry_inode = u32::from_le_bytes([
            dir_data[offset],
            dir_data[offset + 1],
            dir_data[offset + 2],
            dir_data[offset + 3],
        ]);
        let rec_len = u16::from_le_bytes([dir_data[offset + 4], dir_data[offset + 5]]) as usize;
        let entry_name_len = dir_data[offset + 6] as usize;

        if rec_len < MIN_DIR_ENTRY_SIZE {
            return Err("Corrupt directory entry");
        }
        if entry_inode == 0 {
            if rec_len >= new_entry_size {
                return Ok(Some(Room::Empty { offset, rec_len }));
            }
        } else {
            let used = required_entry_size(entry_name_len);
            if rec_len.saturating_sub(used) >= new_entry_size {
                return Ok(Some(Room::Tail { offset, rec_len, used }));
            }
        }
        offset += rec_len;
    }
    Ok(None)
}

/// Write a directory entry at the given offset
fn write_dir_entry(
    dir_data: &mut [u8],
    offset: usize,
    inode: u32,
    rec_len: u16,
    name: &[u8],
    file_type: u8,
) {
    // Write inode
    dir_data[offset..offset + 4].copy_from_slice(&inode.to_le_bytes());
    // Write rec_len
    dir_data[offset + 4..offset + 6].copy_from_slice(&rec_len.to_le_bytes());
    // Write name_len
    dir_data[offset + 6] = name.len() as u8;
    // Write file_type
    dir_data[offset + 7] = file_type;
    // Write name
    dir_data[offset + 8..offset + 8 + name.len()].copy_from_slice(name);
    // Zero-fill the rest of the record for cleanliness
    for i in (offset + 8 + name.len())..(offset + rec_len as usize) {
        if i < dir_data.len() {
            dir_data[i] = 0;
        }
    }
}

/// Check if a directory is empty (contains only "." and ".." entries)
///
/// # Arguments
/// * `dir_data` - Directory data buffer
///
/// # Returns
/// * `true` - Directory is empty (only . and ..)
/// * `false` - Directory has other entries
pub fn is_directory_empty(data: &[u8]) -> bool {
    raw_entries(data).all(|entry| entry.name == b"." || entry.name == b"..")
}

/// Point an existing directory entry at another inode
///
/// Finds the entry with the given name and rewrites its inode number and file
/// type in place: a rename that replaces a name, and `..` of a directory
/// moved to another parent.
///
/// # Arguments
/// * `dir_data` - Mutable directory data buffer
/// * `name` - Name of the entry to update
/// * `new_inode` - New inode number
/// * `file_type` - File type of the new inode (EXT2_FT_* constant)
///
/// # Returns
/// * `Ok(offset)` - Byte offset of the updated entry
/// * `Err(msg)` - Entry not found or other error
pub fn update_directory_entry(
    dir_data: &mut [u8],
    name: &str,
    new_inode: u32,
    file_type: u8,
) -> Result<usize, &'static str> {
    let mut offset = 0usize;

    while offset < dir_data.len() {
        // Ensure we have at least enough bytes for the header
        if offset + MIN_DIR_ENTRY_SIZE > dir_data.len() {
            break;
        }

        // Read existing entry header
        let entry_inode = u32::from_le_bytes([
            dir_data[offset],
            dir_data[offset + 1],
            dir_data[offset + 2],
            dir_data[offset + 3],
        ]);
        let rec_len = u16::from_le_bytes([dir_data[offset + 4], dir_data[offset + 5]]) as usize;
        let entry_name_len = dir_data[offset + 6] as usize;

        if rec_len == 0 || rec_len < MIN_DIR_ENTRY_SIZE {
            return Err("Corrupt directory entry");
        }

        // Check if this is the entry we're looking for
        if entry_inode != 0 && entry_name_len == name.len() {
            let entry_name_offset = offset + MIN_DIR_ENTRY_SIZE;
            if entry_name_offset + entry_name_len <= dir_data.len() {
                let entry_name = &dir_data[entry_name_offset..entry_name_offset + entry_name_len];
                if entry_name == name.as_bytes() {
                    // Found it - update the inode number and type
                    dir_data[offset..offset + 4].copy_from_slice(&new_inode.to_le_bytes());
                    dir_data[offset + 7] = file_type;
                    return Ok(offset);
                }
            }
        }

        offset += rec_len;
    }

    Err("Entry not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a mock directory entry in a buffer
    fn create_dir_entry(buf: &mut [u8], inode: u32, rec_len: u16, name: &str, file_type: u8) {
        // Write header
        buf[0..4].copy_from_slice(&inode.to_le_bytes());
        buf[4..6].copy_from_slice(&rec_len.to_le_bytes());
        buf[6] = name.len() as u8;
        buf[7] = file_type;
        // Write name
        buf[8..8 + name.len()].copy_from_slice(name.as_bytes());
    }

    #[test]
    fn test_single_entry() {
        let mut buf = [0u8; 64];
        create_dir_entry(&mut buf[0..], 42, 16, "test.txt", EXT2_FT_REG_FILE);

        let entries = parse_directory(&buf);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].inode, 42);
        assert_eq!(entries[0].name, "test.txt");
        assert_eq!(entries[0].file_type, EXT2_FT_REG_FILE);
        assert!(entries[0].is_file());
        assert!(!entries[0].is_dir());
    }

    #[test]
    fn test_multiple_entries() {
        let mut buf = [0u8; 128];
        create_dir_entry(&mut buf[0..], 2, 12, ".", EXT2_FT_DIR);
        create_dir_entry(&mut buf[12..], 1, 12, "..", EXT2_FT_DIR);
        create_dir_entry(&mut buf[24..], 10, 20, "file.txt", EXT2_FT_REG_FILE);

        let entries = parse_directory(&buf);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_dot());
        assert!(entries[1].is_dotdot());
        assert_eq!(entries[2].name, "file.txt");
    }

    #[test]
    fn test_deleted_entry() {
        let mut buf = [0u8; 64];
        create_dir_entry(&mut buf[0..], 0, 16, "deleted", EXT2_FT_REG_FILE);

        let entries = parse_directory(&buf);
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_find_entry() {
        let mut buf = [0u8; 128];
        create_dir_entry(&mut buf[0..], 2, 12, ".", EXT2_FT_DIR);
        create_dir_entry(&mut buf[12..], 1, 12, "..", EXT2_FT_DIR);
        create_dir_entry(&mut buf[24..], 10, 20, "target.txt", EXT2_FT_REG_FILE);

        let entry = find_entry(&buf, "target.txt");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().inode, 10);

        let missing = find_entry(&buf, "missing.txt");
        assert!(missing.is_none());
    }
}

/// A borrowed live directory record; iteration allocates no pathname strings.
pub struct RawEntry<'a> {
    pub inode: u32,
    pub file_type: u8,
    pub name: &'a [u8],
    pub offset: usize,
    pub next: usize,
}
pub fn raw_entries(data: &[u8]) -> impl Iterator<Item=RawEntry<'_>> {
    let mut offset = 0usize;
    core::iter::from_fn(move || loop {
        let header = data.get(offset..offset.checked_add(8)?)?;
        let inode = u32::from_le_bytes(header[..4].try_into().ok()?);
        let len = u16::from_le_bytes(header[4..6].try_into().ok()?) as usize;
        let name_len = header[6] as usize;
        if len < 8 || offset.checked_add(len)? > data.len() { return None; }
        let old = offset; offset += len;
        if inode == 0 { continue; }
        if name_len == 0 || name_len + 8 > len { return None; }
        return Some(RawEntry { inode, file_type: header[7], name: &data[old+8..old+8+name_len], offset: old, next: offset });
    })
}
pub fn find_inode(data:&[u8],name:&str)->Option<u32>{ raw_entries(data).find(|entry|entry.name==name.as_bytes()).map(|entry|entry.inode) }

#[cfg(test)]
mod raw_tests {
    use super::*;
    #[test]
    fn borrowed_records_keep_exact_names_and_byte_cookies() {
        let mut data = [0u8;40];
        data[..4].copy_from_slice(&2u32.to_le_bytes()); data[4..6].copy_from_slice(&12u16.to_le_bytes());
        data[6]=1; data[7]=EXT2_FT_DIR; data[8]=b'.';
        data[16..18].copy_from_slice(&12u16.to_le_bytes());
        data[24..28].copy_from_slice(&11u32.to_le_bytes()); data[28..30].copy_from_slice(&16u16.to_le_bytes());
        data[30]=4; data[31]=EXT2_FT_FIFO; data[32..36].copy_from_slice(b"fifo");
        let records: Vec<_> = raw_entries(&data).collect();
        assert_eq!(records.len(),2);
        assert_eq!((records[0].offset,records[0].next,records[0].name),(0,12,b".".as_slice()));
        assert_eq!((records[1].inode,records[1].file_type,records[1].offset,records[1].next),(11,EXT2_FT_FIFO,24,40));
        assert_eq!(find_inode(&data,"fifo"),Some(11));
        data[32]=0xff; assert_eq!(find_inode(&data,"fifo"),None);
        assert_eq!(raw_entries(&data).nth(1).unwrap().name,&[0xff,b'i',b'f',b'o']);
        data[4..6].fill(0); assert!(raw_entries(&data).next().is_none());
    }
}

struct RawLocation { inode:u32,offset:usize,rec_len:u16,prev_offset:Option<usize>,prev_rec_len:Option<u16> }
fn raw_location(data:&[u8],name:&str)->Option<RawLocation>{
    let mut offset=0usize;let mut prev_offset=None;let mut prev_rec_len=None;
    while offset<data.len(){
        let header=data.get(offset..offset.checked_add(8)?)?;
        let inode=u32::from_le_bytes(header[..4].try_into().ok()?);let len=u16::from_le_bytes(header[4..6].try_into().ok()?);
        if len<8||offset.checked_add(len as usize)?>data.len(){return None;}
        let count=header[6] as usize;
        if inode!=0&&count+8<=len as usize&&data.get(offset+8..offset+8+count)?==name.as_bytes(){
            return Some(RawLocation{inode,offset,rec_len:len,prev_offset,prev_rec_len});
        }
        prev_offset=Some(offset);prev_rec_len=Some(len);offset+=len as usize;
    }None
}

pub fn find_inode_bytes(data: &[u8], name: &[u8]) -> Option<u32> {
    raw_entries(data).find(|entry| entry.name == name).map(|entry| entry.inode)
}
