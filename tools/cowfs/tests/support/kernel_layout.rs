// Compile the codecs with CRC32C as a sibling, as in fs::cowfs in the kernel.
#[path = "../../../../kernel/src/fs/cowfs/crc32c.rs"]
pub mod crc32c;
#[path = "../../../../kernel/src/fs/cowfs/format.rs"]
pub mod format;
