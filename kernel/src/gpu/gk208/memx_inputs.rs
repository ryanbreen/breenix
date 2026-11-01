//! C5b read-only inputs missing from the C5a DDR3 calculation snapshot.
//! The caller supplies reads only; no memory/clock programming is performed.
pub fn capture(mut read: impl FnMut(u32) -> u32) -> [(u32, u32); 32] {
    [
        0x10f200, 0x10f224, 0x10f248, 0x10f24c, 0x10f250,
        0x10f290, 0x10f294, 0x10f298, 0x10f29c, 0x10f2a0, 0x10f2a4,
        0x10f2a8, 0x10f2ac, 0x10f2cc, 0x10f2e8, 0x10f300, 0x10f304,
        0x10f320, 0x10f400, 0x10f410, 0x10f420, 0x10f430, 0x10f440,
        0x10f444, 0x10f468, 0x10f808, 0x10f824, 0x10f82c, 0x10f830,
        0x132034, 0x132040, 0x1373ec,
    ].map(|address| (address, read(address)))
}
