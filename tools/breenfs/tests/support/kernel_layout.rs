// Compile the codecs with CRC32C as a sibling, as in fs::breenfs in the kernel.
#[path = "../../../../kernel/src/fs/breenfs/crc32c.rs"]
pub mod crc32c;
#[path = "../../../../kernel/src/fs/breenfs/format.rs"]
pub mod format;
