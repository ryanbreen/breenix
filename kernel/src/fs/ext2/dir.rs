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
    pub inode: u32,        // Inode number (0 = deleted entry)
    pub rec_len: u16,      // Total entry length (includes padding)
    pub name_len: u8,      // Name length
    pub file_type: u8,     // File type (EXT2_FT_*)
}

/// Parsed directory entry with owned name
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub inode: u32,
    pub file_type: u8,
    pub name: String,
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

impl<'a> Iterator for DirReader<'a> {
    type Item = DirEntry;

    fn next(&mut self) -> Option<Self::Item> {
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
            let name_offset = self.offset + header_size;

            // Advance offset to next entry
            self.offset += rec_len as usize;

            // Skip deleted entries (inode == 0)
            if inode == 0 {
                continue;
            }

            // Validate name_len
            if name_len == 0 || name_offset + name_len as usize > self.data.len() {
                // Invalid name length, stop iteration
                return None;
            }

            // Extract name bytes
            let name_bytes = &self.data[name_offset..name_offset + name_len as usize];

            // Convert to String, replacing invalid UTF-8 with replacement char
            let name = String::from_utf8_lossy(name_bytes).into_owned();

            return Some(DirEntry {
                inode,
                file_type,
                name,
            });
        }
    }
}

/// Parse all directory entries from a data buffer
pub fn parse_directory(data: &[u8]) -> Vec<DirEntry> {
    DirReader::new(data).collect()
}

/// Find a specific entry by name in directory data
pub fn find_entry(data: &[u8], name: &str) -> Option<DirEntry> {
    DirReader::new(data).find(|e| e.name == name)
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
///
/// # Returns
/// * `Ok(u32)` - Inode number of the removed entry
/// * `Err(&'static str)` - Error message
pub fn remove_entry(data: &mut [u8], name: &str) -> Result<u32, &'static str> {
    // Cannot remove . or ..
    if name == "." || name == ".." {
        return Err("Cannot remove . or ..");
    }

    // Find the entry location
    let location = find_entry_location(data, name).ok_or("Entry not found")?;
    let removed_inode = location.entry.inode;

    // If there's a previous entry, extend its rec_len to include this entry
    // (effectively "absorbing" this entry's space)
    if let (Some(prev_off), Some(prev_rec)) = (location.prev_offset, location.prev_rec_len) {
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
/// * `Ok(())` - Entry was added successfully
/// * `Err(msg)` - No space available or other error
pub fn add_directory_entry(
    dir_data: &mut Vec<u8>,
    new_inode: u32,
    name: &str,
    file_type: u8,
) -> Result<(), &'static str> {
    let name_bytes = name.as_bytes();
    if name_bytes.is_empty() || name_bytes.len() > 255 {
        return Err("Invalid name length");
    }

    let new_entry_size = required_entry_size(name_bytes.len());
    let mut offset = 0usize;

    // Search for space in existing entries
    while offset < dir_data.len() {
        // Ensure we can read the header
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
        let rec_len = u16::from_le_bytes([
            dir_data[offset + 4],
            dir_data[offset + 5],
        ]) as usize;
        let entry_name_len = dir_data[offset + 6] as usize;

        if rec_len == 0 || rec_len < MIN_DIR_ENTRY_SIZE {
            return Err("Corrupt directory entry");
        }

        if entry_inode == 0 {
            // Deleted entry - check if we can reuse its space
            if rec_len >= new_entry_size {
                // Reuse this entry
                write_dir_entry(dir_data, offset, new_inode, rec_len as u16, name_bytes, file_type);
                return Ok(());
            }
        } else {
            // Active entry - check if we can split its padding
            let actual_size = required_entry_size(entry_name_len);
            let available_space = rec_len.saturating_sub(actual_size);

            if available_space >= new_entry_size {
                // Split this entry - shrink existing entry and use the rest
                let existing_new_rec_len = actual_size as u16;
                let new_entry_rec_len = (rec_len - actual_size) as u16;

                // Update existing entry's rec_len
                dir_data[offset + 4] = existing_new_rec_len as u8;
                dir_data[offset + 5] = (existing_new_rec_len >> 8) as u8;

                // Write new entry after the existing one
                let new_offset = offset + actual_size;
                write_dir_entry(dir_data, new_offset, new_inode, new_entry_rec_len, name_bytes, file_type);
                return Ok(());
            }
        }

        offset += rec_len;
    }

    // No space found in existing entries - need to extend the directory
    // This would require allocating a new data block, which is complex
    Err("No space in directory")
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
pub fn is_directory_empty(dir_data: &[u8]) -> bool {
    for entry in DirReader::new(dir_data) {
        // Skip "." and ".." entries
        if entry.name == "." || entry.name == ".." {
            continue;
        }
        // Found a non-dot entry
        return false;
    }
    // Only found . and .. (or no entries at all)
    true
}

/// Update a directory entry's inode number (for updating ".." when moving directories)
///
/// Finds the entry with the given name and updates its inode number.
///
/// # Arguments
/// * `dir_data` - Mutable directory data buffer
/// * `name` - Name of the entry to update
/// * `new_inode` - New inode number
///
/// # Returns
/// * `Ok(())` - Entry was updated
/// * `Err(msg)` - Entry not found or other error
pub fn update_directory_entry(dir_data: &mut [u8], name: &str, new_inode: u32) -> Result<(), &'static str> {
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
        let rec_len = u16::from_le_bytes([
            dir_data[offset + 4],
            dir_data[offset + 5],
        ]) as usize;
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
                    // Found it - update the inode number
                    dir_data[offset..offset + 4].copy_from_slice(&new_inode.to_le_bytes());
                    return Ok(());
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
