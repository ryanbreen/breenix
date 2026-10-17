use super::quota::Charge;
use super::{Pages, ENOMEM, PAGE};

/// An upload's or readback's staged rectangle: whole frames charged to the process's GPU quota (the
/// guest-backing quota and `RLIMIT_AS`, as resource backing is), rather than heap pixel buffers.
/// Frame descriptors remain heap metadata. The staging is CPU-only and freed with the call; its
/// frames go before its charge.
pub(super) struct Staging {
    pub(super) pages: Pages,
    _charge: Charge,
}

impl Staging {
    pub(super) fn new(tgid: u64, bytes: usize) -> Result<Staging, i64> {
        let frames = bytes.div_ceil(PAGE);
        let charge = Charge::new(tgid, (frames * PAGE) as u64, false)?;
        let pages = Pages::new(frames).ok_or(ENOMEM)?;
        Ok(Staging { pages, _charge: charge })
    }
}
