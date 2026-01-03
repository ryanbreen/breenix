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
