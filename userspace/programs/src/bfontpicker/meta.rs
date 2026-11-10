//! Font classification from SFNT metadata.

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?))
}

fn table<'a>(bytes: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let count = usize::from(u16_at(bytes, 4)?);
    for index in 0..count {
        let record = 12usize.checked_add(index.checked_mul(16)?)?;
        if bytes.get(record..record.checked_add(4)?)? == tag {
            let start = usize::try_from(u32_at(bytes, record + 8)?).ok()?;
            let len = usize::try_from(u32_at(bytes, record + 12)?).ok()?;
            return bytes.get(start..start.checked_add(len)?);
        }
    }
    None
}

/// A readable post table is required; OS/2 PANOSE supplements its fixed-pitch flag.
pub fn is_mono(bytes: &[u8]) -> Result<bool, &'static str> {
    let post = table(bytes, b"post").ok_or("missing or unreadable post table")?;
    let fixed = u32_at(post, 12).ok_or("missing or unreadable post table")? != 0;
    let panose_mono = table(bytes, b"OS/2").and_then(|os2| os2.get(35)).copied() == Some(9);
    Ok(fixed || panose_mono)
}
