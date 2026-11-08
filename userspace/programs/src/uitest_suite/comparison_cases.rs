use libuitest::compare::{compare, Error, Frame, Format, Options, Rect, Tolerance};

type Result = core::result::Result<(), String>;
fn frame(bytes: &[u8], w: usize, h: usize) -> Frame<'_> {
    Frame::new(bytes, w, h, w * 4, Format::Bgrx).unwrap()
}
fn options(channel: u8, max_differing: usize) -> Options<'static> {
    Options { tolerance: Tolerance { channel, max_differing }, reason: "synthetic test", ..Options::default() }
}
pub fn exact_match() -> Result {
    let p = [0u8; 16];
    let r = compare(frame(&p, 2, 2), frame(&p, 2, 2), Options::default()).map_err(|e| format!("{e:?}"))?;
    if !r.comparison.passed() || r.comparison.differing != 0 { return Err("identical frames differ".into()); }
    Ok(())
}
pub fn one_pixel_fails() -> Result {
    let p = [0u8; 16]; let mut q = p; q[8..12].copy_from_slice(&[3, 4, 5, 0]);
    let r = compare(frame(&p, 2, 2), frame(&q, 2, 2), Options::default()).map_err(|e| format!("{e:?}"))?;
    let d = r.comparison.first.ok_or("missing difference")?;
    if r.comparison.passed() || (d.x, d.y, d.expected, d.actual) != (0, 1, [0,0,0,255], [3,4,5,255]) { return Err(format!("wrong mismatch: {r:?}")); }
    Ok(())
}
pub fn channel_tolerance() -> Result {
    let p = [0u8; 4]; let q = [0, 2, 0, 0];
    for (channel, expected) in [(2, true), (1, false)] {
        let r = compare(frame(&p, 1, 1), frame(&q, 1, 1), options(channel, 0)).map_err(|e| format!("{e:?}"))?;
        if r.comparison.passed() != expected { return Err("channel tolerance boundary".into()); }
    } Ok(())
}
pub fn max_differing() -> Result {
    let p = vec![0u8; 2000 * 4]; let mut q = p.clone(); q[0] = 1; q[4] = 1;
    for (count, expected) in [(1, true), (2, false)] {
        q[4] = if count == 1 {0} else {1};
        let r = compare(frame(&p, 2000, 1), frame(&q, 2000, 1), options(0, 1)).map_err(|e| format!("{e:?}"))?;
        if r.comparison.differing != count || r.comparison.passed() != expected { return Err("pixel cap boundary".into()); }
    } Ok(())
}
pub fn regions_same_frame() -> Result {
    let p = [0u8; 16]; let mut a = p; let mut b = p; a[0]=1; b[8]=1;
    let regions = [Rect{x:0,y:0,w:2,h:1}, Rect{x:0,y:1,w:2,h:1}];
    let opts = Options { regions: &regions, ..Options::default() };
    for image in [&a, &b] {
        let r = compare(frame(&p,2,2), frame(image,2,2), opts).map_err(|e| format!("{e:?}"))?;
        if r.comparison.passed() { return Err("regions passed across different frames".into()); }
    }
    let r = compare(frame(&p,2,2), frame(&a,2,2), opts).map_err(|e| format!("{e:?}"))?;
    if r.closest_region != Some(0) { return Err(format!("wrong closest region: {r:?}")); }
    Ok(())
}
pub fn size_mismatch() -> Result {
    let p = [0u8; 16];
    if !matches!(compare(frame(&p,2,2), frame(&p,1,2), Options::default()), Err(Error::SizeMismatch { .. })) { return Err("unequal dimensions accepted".into()); }
    Ok(())
}
pub fn compare_cost() -> Result {
    let p = vec![0u8; 1280 * 800 * 4]; let mut q = p.clone(); q[0] = 1;
    let start = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?;
    let result = compare(frame(&p,1280,800), frame(&q,1280,800), Options::default()).map_err(|e| format!("{e:?}"))?;
    let end = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?;
    if result.comparison.passed() || (end.as_nanos() - start.as_nanos()) > 50_000_000 { return Err("comparison failed or exceeded 50 ms".into()); }
    Ok(())
}
