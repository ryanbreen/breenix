pub struct Rate {
    pub intervals: u64,
    pub span_ns: u64,
    pub passes: bool,
}

pub fn measure(times: &[u64]) -> Option<Rate> {
    if times.len() < 2 {
        return None;
    }
    let first = *times.iter().min()?;
    let last = *times.iter().max()?;
    let span_ns = last - first;
    if span_ns == 0 {
        return None;
    }
    let intervals = (times.len() - 1) as u64;
    Some(Rate {
        intervals,
        span_ns,
        passes: u128::from(intervals) * 1_000_000_000 >= 60 * u128::from(span_ns),
    })
}
