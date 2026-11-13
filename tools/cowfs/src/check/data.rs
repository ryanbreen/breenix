//! C1 in full mode: each data block matches the CRC its REF item records (format 2.4).

use std::io;

use super::image::Image;
use super::raw::BLOCK;
use super::report::Report;
use super::walk::Walk;

pub fn verify(image: &Image, walk: &Walk<'_>, report: &mut Report) -> io::Result<()> {
    for r in &walk.data_refs {
        let name = format!("data REF {}+{}", r.start, r.length);
        if r.start.checked_add(r.length).is_none_or(|end| end > image.blocks) {
            report.error("C1", format!("{name} runs past the image's {} blocks", image.blocks));
            continue;
        }
        let blocks = image.read(r.start, r.length)?;
        for (i, (data, &want)) in blocks.chunks_exact(BLOCK).zip(&r.crcs).enumerate() {
            let got = cowfs::crc32c::checksum(data);
            if got != want {
                let block = r.start + i as u64;
                report.error("C1", format!("block {block} ({name}): CRC {got:#010x}, REF records {want:#010x}"));
            }
        }
    }
    Ok(())
}
