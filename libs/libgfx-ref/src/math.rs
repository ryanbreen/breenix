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

/// The floor of the square root of `n`, exactly, by the digit-by-digit method (at most 32
/// iterations, each counted on `libgfx::work`).
pub fn isqrt_u64(n: u64) -> u64 {
    let mut rem = n;
    let mut root = 0u64;
    let mut bit = 1u64 << 62;
    let mut steps = 0u64;
    while bit > rem {
        steps += 1;
        bit >>= 2;
    }
    while bit != 0 {
        steps += 1;
        if rem >= root + bit {
            rem -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    crate::work::add(steps);
    root
}
