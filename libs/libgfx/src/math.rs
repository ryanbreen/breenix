//! Math utilities.

/// Integer square root using Newton's method.
pub fn isqrt_i64(n: i64) -> i64 {
    if n < 0 {
        return 0;
    }
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}
