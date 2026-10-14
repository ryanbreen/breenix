//! Binary floating formatting using exact decimal integer arithmetic, without libc/libm code.
//! x = significand * 2^e; for negative e, x = significand * 5^(-e) / 10^(-e).
//! The digits are an integer N in base 10^9 limbs, so no conversion rounds before the
//! single decimal rounding, and nothing is allocated: snprintf never fails for memory.
use super::{
    args::Binary,
    format::{field, Body, Layout, Sink},
};

/// Limb base. A limb times a multiplier of at most 2^32, plus a carry below 2^32,
/// stays below 2^64.
const BASE: u64 = 1_000_000_000;
const POW10: [u32; 9] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
];
/// binary64: the largest N is the smallest subnormal's 5^1074 times a 53-bit
/// significand, 751 + 16 = 767 digits, which is 86 limbs.
const DOUBLE_LIMBS: usize = 90;
/// binary128 (aarch64 long double): 5^16494 times a 113-bit significand is
/// 11530 + 35 = 11565 digits, 1285 limbs; the x87 format needs fewer (11514 digits).
/// 1290 limbs are 5160 bytes of stack, used only by long double conversions, so a
/// %Lf on a PTHREAD_STACK_MIN (16384) thread stack leaves room for its callers.
const LONG_LIMBS: usize = 1290;

fn rounding() -> u32 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let v: u64;
        core::arch::asm!("mrs {}, fpcr", out(reg) v, options(nomem, nostack, preserves_flags));
        ((v >> 22) & 3) as u32
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut v = 0u32;
        core::arch::asm!("stmxcsr [{}]", in(reg) &mut v, options(nostack, preserves_flags));
        match (v >> 13) & 3 {
            1 => 2,
            2 => 1,
            3 => 3,
            _ => 0,
        }
    }
}
fn up(first: u8, sticky: bool, odd: bool, negative: bool, mode: u32) -> bool {
    match mode {
        1 => !negative && (first != 0 || sticky),
        2 => negative && (first != 0 || sticky),
        3 => false,
        _ => first > 5 || (first == 5 && (sticky || odd)),
    }
}
fn decimal_digits(v: u32) -> usize {
    let mut n = 1;
    while n < 9 && v >= POW10[n] {
        n += 1;
    }
    n
}
/// The value is N * 10^(e - n + 1): N has exactly n decimal digits (N = 0 has one),
/// and e is the decimal exponent of its leading digit.
struct Decimal<const L: usize> {
    limb: [u32; L],
    len: usize,
    n: usize,
    e: i32,
}
impl<const L: usize> Decimal<L> {
    fn new(value: Binary) -> Self {
        let mut s = Self {
            limb: [0; L],
            len: 1,
            n: 1,
            e: 0,
        };
        let mut sig = value.significand;
        let mut i = 0;
        while sig != 0 {
            s.limb[i] = (sig % BASE as u128) as u32;
            sig /= BASE as u128;
            i += 1;
        }
        if i == 0 {
            return s;
        }
        s.len = i;
        let mut k = value.exponent;
        while k > 0 {
            let step = k.min(32);
            s.multiply(1u64 << step);
            k -= step;
        }
        while k < 0 {
            let step = (-k).min(13);
            s.multiply(5u64.pow(step as u32));
            k += step;
        }
        s.n = s.count();
        s.e = s.n as i32 - 1 + value.exponent.min(0);
        s
    }
    fn count(&self) -> usize {
        decimal_digits(self.limb[self.len - 1]) + 9 * (self.len - 1)
    }
    fn multiply(&mut self, m: u64) {
        let mut carry = 0u64;
        for limb in &mut self.limb[..self.len] {
            let v = *limb as u64 * m + carry;
            *limb = (v % BASE) as u32;
            carry = v / BASE;
        }
        while carry != 0 {
            self.limb[self.len] = (carry % BASE) as u32;
            carry /= BASE;
            self.len += 1;
        }
    }
    fn normalize(&mut self) {
        while self.len > 1 && self.limb[self.len - 1] == 0 {
            self.len -= 1;
        }
    }
    /// N /= d; returns the remainder.
    fn divide(&mut self, d: u32) -> u32 {
        let mut rem = 0u64;
        for limb in self.limb[..self.len].iter_mut().rev() {
            let v = rem * BASE + *limb as u64;
            *limb = (v / d as u64) as u32;
            rem = v % d as u64;
        }
        self.normalize();
        rem as u32
    }
    /// Drops the m lowest digits; returns whether any of them was nonzero.
    fn shift(&mut self, m: usize) -> bool {
        let whole = (m / 9).min(self.len);
        let mut sticky = self.limb[..whole].iter().any(|&x| x != 0);
        if whole == self.len {
            self.limb[0] = 0;
            self.len = 1;
            return sticky;
        }
        self.limb.copy_within(whole..self.len, 0);
        self.len -= whole;
        if m % 9 != 0 {
            sticky |= self.divide(POW10[m % 9]) != 0;
        }
        sticky
    }
    fn increment(&mut self) {
        for i in 0..self.len {
            if (self.limb[i] as u64) < BASE - 1 {
                self.limb[i] += 1;
                return;
            }
            self.limb[i] = 0;
        }
        self.limb[self.len] = 1;
        self.len += 1;
    }
    fn set(&mut self, v: u32) {
        self.limb[0] = v;
        self.len = 1;
        self.n = 1;
    }
    /// Rounds to `keep` significant digits in the given direction.
    fn round(&mut self, keep: i32, negative: bool, mode: u32) {
        if keep >= self.n as i32 {
            return;
        }
        if keep <= 0 {
            // Nothing is kept: the result is zero or one unit in the place before.
            let first = if keep == 0 { self.digit(0) - b'0' } else { 0 };
            let sticky = if keep == 0 {
                self.shift(self.n - 1)
            } else {
                self.limb[..self.len].iter().any(|&x| x != 0)
            };
            if up(first, sticky, false, negative, mode) {
                self.set(1);
                self.e += 1 - keep;
            } else {
                self.set(0);
                self.e = 0;
            }
            return;
        }
        let keep = keep as usize;
        let sticky = self.shift(self.n - keep - 1);
        let first = self.divide(10) as u8;
        self.n = keep;
        if up(first, sticky, self.limb[0] & 1 != 0, negative, mode) {
            self.increment();
            if self.count() > keep {
                self.divide(10);
                self.e += 1;
            }
        }
    }
    fn digit(&self, i: i64) -> u8 {
        if i < 0 || i >= self.n as i64 {
            return b'0';
        }
        let pos = self.n - 1 - i as usize;
        b'0' + ((self.limb[pos / 9] / POW10[pos % 9]) % 10) as u8
    }
    /// Index of the last nonzero digit (0 for zero).
    fn last_nonzero(&self) -> usize {
        let mut zeros = 0;
        for &limb in &self.limb[..self.len] {
            if limb == 0 {
                zeros += 9;
                continue;
            }
            let mut v = limb;
            while v % 10 == 0 {
                v /= 10;
                zeros += 1;
            }
            break;
        }
        self.n.saturating_sub(zeros + 1)
    }
    /// Writes digits first..first+count; positions outside the n digits are zeros.
    fn digits(&self, out: &mut dyn Body, first: i64, count: usize) -> Result<(), i32> {
        let lead = (-first).clamp(0, count as i64) as usize;
        out.zeros(lead)?;
        let start = first + lead as i64;
        let real = (self.n as i64 - start).clamp(0, (count - lead) as i64) as usize;
        let mut chunk = [0u8; 64];
        let mut done = 0;
        while done < real {
            let k = (real - done).min(chunk.len());
            for (j, c) in chunk[..k].iter_mut().enumerate() {
                *c = self.digit(start + (done + j) as i64);
            }
            out.put(&chunk[..k])?;
            done += k;
        }
        out.zeros(count - lead - real)
    }
}
fn exponent(out: &mut dyn Body, e: i32, letter: u8, minimum: usize) -> Result<(), i32> {
    let mut b = [0u8; 16];
    let mut n = 0;
    let mut v = e.unsigned_abs();
    loop {
        b[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while n < minimum {
        b[n] = b'0';
        n += 1;
    }
    b[n] = if e < 0 { b'-' } else { b'+' };
    b[n + 1] = letter;
    n += 2;
    b[..n].reverse();
    out.put(&b[..n])
}
/// Formats one %f %e %g %a conversion (either case) as a complete field.
pub(super) fn convert(
    out: &mut dyn Sink,
    layout: Layout,
    sign: &[u8],
    value: Binary,
    conversion: u8,
    precision: Option<usize>,
    alternate: bool,
) -> Result<(), i32> {
    let upper = conversion.is_ascii_uppercase();
    let c = conversion.to_ascii_lowercase();
    if value.special != 0 {
        let text: &[u8] = match (value.special, upper) {
            (1, false) => b"inf",
            (1, true) => b"INF",
            (_, false) => b"nan",
            (_, true) => b"NAN",
        };
        let layout = Layout {
            zero: false,
            ..layout
        };
        return field(out, layout, sign, &mut |b| b.put(text));
    }
    let mode = rounding();
    if c == b'a' {
        return hexadecimal(out, layout, sign, value, upper, precision, alternate, mode);
    }
    if value.fraction_bits <= 52 {
        decimal::<DOUBLE_LIMBS>(
            out, layout, sign, value, c, upper, precision, alternate, mode,
        )
    } else {
        decimal::<LONG_LIMBS>(
            out, layout, sign, value, c, upper, precision, alternate, mode,
        )
    }
}
#[allow(clippy::too_many_arguments)]
fn hexadecimal(
    out: &mut dyn Sink,
    layout: Layout,
    sign: &[u8],
    value: Binary,
    upper: bool,
    precision: Option<usize>,
    alternate: bool,
    mode: u32,
) -> Result<(), i32> {
    let neg = value.negative;
    let padding = (4 - value.fraction_bits % 4) % 4;
    let hex_digits = (value.fraction_bits as usize + 3) / 4;
    let mut fraction = (value.significand & ((1u128 << value.fraction_bits) - 1)) << padding;
    let mut lead = (value.significand >> value.fraction_bits) as u8;
    let e = if value.significand == 0 {
        0
    } else {
        value.exponent + value.fraction_bits as i32
    };
    let p = precision.unwrap_or_else(|| {
        let mut p = hex_digits;
        while p > 0 && ((fraction >> ((hex_digits - p) * 4)) & 15) == 0 {
            p -= 1;
        }
        p
    });
    if p < hex_digits {
        let shift = (hex_digits - p) * 4;
        let mask = (1u128 << shift) - 1;
        let rest = fraction & mask;
        let halfway = 1u128 << (shift - 1);
        let odd = if p == 0 {
            lead & 1 != 0
        } else {
            (fraction >> shift) & 1 != 0
        };
        let inc = match mode {
            1 => !neg && rest != 0,
            2 => neg && rest != 0,
            3 => false,
            _ => rest > halfway || (rest == halfway && odd),
        };
        fraction &= !mask;
        if inc {
            fraction += 1u128 << shift;
            if fraction >> (hex_digits * 4) != 0 {
                lead += 1;
                fraction = 0;
            }
        }
    }
    let alphabet = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut prefix = [0u8; 3];
    prefix[..sign.len()].copy_from_slice(sign);
    prefix[sign.len()..sign.len() + 2].copy_from_slice(if upper { b"0X" } else { b"0x" });
    let prefix = &prefix[..sign.len() + 2];
    field(out, layout, prefix, &mut |b| {
        b.put(&[b'0' + lead])?;
        if p > 0 || alternate {
            b.put(b".")?;
        }
        let shown = p.min(hex_digits);
        let mut chunk = [0u8; 32];
        for (i, c) in chunk[..shown].iter_mut().enumerate() {
            *c = alphabet[((fraction >> ((hex_digits - 1 - i) * 4)) & 15) as usize];
        }
        b.put(&chunk[..shown])?;
        b.zeros(p - shown)?;
        exponent(b, e, if upper { b'P' } else { b'p' }, 1)
    })
}
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn decimal<const L: usize>(
    out: &mut dyn Sink,
    layout: Layout,
    sign: &[u8],
    value: Binary,
    c: u8,
    upper: bool,
    precision: Option<usize>,
    alternate: bool,
    mode: u32,
) -> Result<(), i32> {
    let neg = value.negative;
    let mut d = Decimal::<L>::new(value);
    let mut p = precision.unwrap_or(6);
    let mut scientific = c == b'e';
    if c == b'g' {
        p = core::cmp::max(p, 1);
        d.round(i32::try_from(p).map_err(|_| 75)?, neg, mode);
        scientific = d.e < -4 || d.e >= p as i32;
        p = if scientific {
            p - 1
        } else {
            (p as i64 - d.e as i64 - 1).max(0) as usize
        };
        if !alternate {
            let last = d.last_nonzero();
            let needed = if scientific {
                last
            } else {
                (last as i32 - d.e).max(0) as usize
            };
            p = p.min(needed);
        }
    } else {
        let keep = if scientific {
            p as i64 + 1
        } else {
            p as i64 + d.e as i64 + 1
        };
        d.round(i32::try_from(keep).map_err(|_| 75)?, neg, mode);
    }
    let point = p > 0 || alternate;
    field(out, layout, sign, &mut |b| {
        if scientific {
            d.digits(b, 0, 1)?;
            if point {
                b.put(b".")?;
            }
            d.digits(b, 1, p)?;
            exponent(b, d.e, if upper { b'E' } else { b'e' }, 2)
        } else {
            let integer = d.e.max(0) as usize + 1;
            d.digits(b, d.e as i64 - (integer as i64 - 1), integer)?;
            if point {
                b.put(b".")?;
            }
            d.digits(b, d.e as i64 + 1, p)
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Collect(std::vec::Vec<u8>);
    impl Sink for Collect {
        fn bytes(&mut self, b: &[u8]) -> bool {
            self.0.extend_from_slice(b);
            true
        }
        fn wide(&mut self, _: u32) -> bool {
            false
        }
    }
    fn render(v: Binary, c: u8, precision: Option<usize>) -> std::vec::Vec<u8> {
        let mut out = Collect(std::vec::Vec::new());
        let layout = Layout {
            width: 0,
            left: false,
            zero: false,
        };
        convert(&mut out, layout, b"", v, c, precision, false).unwrap();
        out.0
    }
    #[test]
    fn binary128_significand_is_not_downcast() {
        let v = Binary {
            negative: false,
            significand: (1u128 << 112) + 1,
            exponent: -112,
            fraction_bits: 112,
            special: 0,
        };
        assert_eq!(
            render(v, b'f', Some(36)),
            b"1.000000000000000000000000000000000193"
        );
        assert_eq!(
            render(v, b'a', None),
            b"0x1.0000000000000000000000000001p+0"
        );
    }
    #[test]
    fn binary128_subnormal_exact_digits() {
        let v = Binary {
            negative: false,
            significand: 1,
            exponent: -16494,
            fraction_bits: 112,
            special: 0,
        };
        let start = std::time::Instant::now();
        assert_eq!(
            render(v, b'g', Some(35)),
            b"6.4751751194380251109244389582276466e-4966"
        );
        // The smallest binary128 subnormal has 11530 significant digits; all of them
        // are produced exactly, and %f prints 16494 places of which the last is 5.
        let full = render(v, b'f', Some(16494));
        assert_eq!(full.len(), 2 + 16494);
        assert_eq!(*full.last().unwrap(), b'5');
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
    #[test]
    fn long_double_conversion_fits_a_minimum_thread_stack() {
        // PTHREAD_STACK_MIN in include/limits.h.
        let worker = std::thread::Builder::new().stack_size(16384).spawn(|| {
            let v = Binary {
                negative: false,
                significand: 1,
                exponent: -16494,
                fraction_bits: 112,
                special: 0,
            };
            render(v, b'e', Some(20)).len()
        });
        assert_eq!(worker.unwrap().join().unwrap(), 28);
    }
    #[test]
    fn largest_binary128_has_4933_integer_digits() {
        let v = Binary {
            negative: false,
            significand: (1u128 << 113) - 1,
            exponent: 16383 - 112,
            fraction_bits: 112,
            special: 0,
        };
        let text = render(v, b'f', Some(0));
        assert_eq!(text.len(), 4933);
        assert!(text.starts_with(b"1189731495357231765085759326628007016196"));
    }
}
