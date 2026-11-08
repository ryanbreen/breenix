use libuitest::compare::{compare, Frame, Format, Options};

fn frame(bytes: &[u8], w: usize, h: usize) -> Frame<'_> { Frame::new(bytes, w, h, w * 4, Format::Bgrx).unwrap() }
pub fn compare_cost() -> Result<(), String> {
    let p = vec![0u8; 1280 * 800 * 4]; let mut q = p.clone(); q[0] = 1;
    let start = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?;
    let result = compare(frame(&p,1280,800), frame(&q,1280,800), Options::default()).map_err(|e| format!("{e:?}"))?;
    let end = libbreenix::time::now_monotonic().map_err(|e| e.to_string())?;
    let elapsed = end.as_nanos() - start.as_nanos();
    breenix_suite::suite::value("compare_ns", elapsed as i64, "ns", Some((0, 500_000_000)));
    if result.comparison.passed() || elapsed > 500_000_000 { return Err("comparison failed or exceeded 500 ms".into()); }
    Ok(())
}
