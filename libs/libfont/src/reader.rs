//! Bounds-checked big-endian reads. Every read past the data returns `None`; callers turn
//! that into the error their scope gives (section 10).

#[inline]
pub(crate) fn u8_at(d: &[u8], o: usize) -> Option<u8> {
    d.get(o).copied()
}

#[inline]
pub(crate) fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    let b = d.get(o..o.checked_add(2)?)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

#[inline]
pub(crate) fn i16_at(d: &[u8], o: usize) -> Option<i16> {
    u16_at(d, o).map(|v| v as i16)
}

#[inline]
pub(crate) fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    let b = d.get(o..o.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// A cursor over `data`.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn at(data: &'a [u8], pos: usize) -> Reader<'a> {
        Reader { data, pos }
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        let v = u8_at(self.data, self.pos)?;
        self.pos += 1;
        Some(v)
    }

    pub(crate) fn i8(&mut self) -> Option<i8> {
        self.u8().map(|v| v as i8)
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        let v = u16_at(self.data, self.pos)?;
        self.pos += 2;
        Some(v)
    }

    pub(crate) fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Option<()> {
        let end = self.pos.checked_add(n)?;
        if end > self.data.len() {
            return None;
        }
        self.pos = end;
        Some(())
    }
}
