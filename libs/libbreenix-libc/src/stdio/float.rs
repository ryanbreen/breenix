//! Binary64 formatting using exact decimal integer arithmetic, without libc/libm code.
//! x = significand * 2^e; for negative e, x = significand * 5^(-e) / 10^(-e).
//! At most 1074 fractional places and 309 integer places are required.
use super::{args::Binary, format::Text};

fn rounding() -> u32 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let v: u64;
        core::arch::asm!("mrs {}, fpcr",out(reg)v,options(nomem,nostack,preserves_flags));
        ((v >> 22) & 3) as u32
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut v = 0u32;
        core::arch::asm!("stmxcsr [{}]",in(reg)&mut v,options(nostack,preserves_flags));
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
struct Decimal {
    d: [u8; 16500],
    n: usize,
    e: i32,
}
impl Decimal {
    fn new(value: Binary) -> Self {
        let mut sig = value.significand;
        let binary = value.exponent;
        let mut s = Self {
            d: [0; 16500],
            n: 0,
            e: 0,
        };
        loop {
            s.d[s.n] = (sig % 10) as u8;
            s.n += 1;
            sig /= 10;
            if sig == 0 {
                break;
            }
        }
        for _ in 0..binary.unsigned_abs() {
            let multiplier = if binary < 0 { 5 } else { 2 };
            let mut carry = 0;
            for i in 0..s.n {
                let v = s.d[i] * multiplier + carry;
                s.d[i] = v % 10;
                carry = v / 10;
            }
            if carry != 0 {
                s.d[s.n] = carry;
                s.n += 1;
            }
        }
        s.d[..s.n].reverse();
        s.e = s.n as i32 - 1 + core::cmp::min(binary, 0);
        if value.significand == 0 {
            s.n = 1;
            s.d[0] = 0;
            s.e = 0;
        }
        s
    }
    fn round(&mut self, keep: i32, negative: bool, mode: u32) {
        if keep >= self.n as i32 {
            return;
        }
        if keep <= 0 {
            let first = if keep == 0 { self.d[0] } else { 0 };
            let sticky = if keep == 0 {
                self.d[1..self.n].iter().any(|&x| x != 0)
            } else {
                self.d[..self.n].iter().any(|&x| x != 0)
            };
            if up(first, sticky, false, negative, mode) {
                self.e += 1 - keep;
                self.d[0] = 1;
            } else {
                self.e = 0;
                self.d[0] = 0;
            }
            self.n = 1;
            return;
        }
        let k = keep as usize;
        let increment = up(
            self.d[k],
            self.d[k + 1..self.n].iter().any(|&x| x != 0),
            self.d[k - 1] & 1 != 0,
            negative,
            mode,
        );
        self.n = k;
        if increment {
            let mut i = k;
            loop {
                if i == 0 {
                    self.d[0] = 1;
                    for c in &mut self.d[1..k] {
                        *c = 0;
                    }
                    self.e += 1;
                    break;
                }
                i -= 1;
                if self.d[i] < 9 {
                    self.d[i] += 1;
                    break;
                }
                self.d[i] = 0;
            }
        }
    }
    fn digit(&self, p: i32) -> u8 {
        if p >= 0 && (p as usize) < self.n {
            self.d[p as usize] + b'0'
        } else {
            b'0'
        }
    }
}
fn exponent(out: &mut Text, e: i32, letter: u8, minimum: usize) -> Result<(), i32> {
    out.byte(letter)?;
    out.byte(if e < 0 { b'-' } else { b'+' })?;
    let mut b = [0; 12];
    let mut n = 0;
    let mut e = e.unsigned_abs();
    loop {
        b[n] = b'0' + (e % 10) as u8;
        n += 1;
        e /= 10;
        if e == 0 {
            break;
        }
    }
    for _ in n..minimum {
        out.byte(b'0')?
    }
    for i in (0..n).rev() {
        out.byte(b[i])?
    }
    Ok(())
}
pub(super) fn render(
    out: &mut Text,
    value: Binary,
    conversion: u8,
    precision: Option<usize>,
    alternate: bool,
) -> Result<(), i32> {
    let neg = value.negative;
    let upper = conversion.is_ascii_uppercase();
    let c = conversion.to_ascii_lowercase();
    if value.special != 0 {
        return out.bytes(if value.special == 1 {
            if upper {
                b"INF"
            } else {
                b"inf"
            }
        } else if upper {
            b"NAN"
        } else {
            b"nan"
        });
    }
    let mode = rounding();
    if c == b'a' {
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
        out.bytes(if upper { b"0X" } else { b"0x" })?;
        out.byte(b'0' + lead)?;
        if p > 0 || alternate {
            out.byte(b'.')?
        }
        let alphabet = if upper {
            b"0123456789ABCDEF"
        } else {
            b"0123456789abcdef"
        };
        for i in 0..p {
            out.byte(if i < hex_digits {
                alphabet[((fraction >> ((hex_digits - 1 - i) * 4)) & 15) as usize]
            } else {
                b'0'
            })?
        }
        return exponent(out, e, if upper { b'P' } else { b'p' }, 1);
    }
    let mut d = Decimal::new(value);
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
            let last = d.d[..d.n]
                .iter()
                .rposition(|&digit| digit != 0)
                .unwrap_or(0);
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
    if scientific {
        out.byte(d.digit(0))?;
        if p > 0 || alternate {
            out.byte(b'.')?
        }
        for i in 1..=p {
            out.byte(d.digit(i as i32))?
        }
        exponent(out, d.e, if upper { b'E' } else { b'e' }, 2)
    } else {
        for exponent in (0..=core::cmp::max(d.e, 0)).rev() {
            out.byte(d.digit(d.e - exponent))?
        }
        if p > 0 || alternate {
            out.byte(b'.')?
        }
        for i in 1..=p {
            out.byte(d.digit(d.e + i as i32))?
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary128_significand_is_not_downcast() {
        let v = Binary {
            negative: false,
            significand: (1u128 << 112) + 1,
            exponent: -112,
            fraction_bits: 112,
            special: 0,
        };
        let mut text = Text::new();
        render(&mut text, v, b'f', Some(36), false).unwrap();
        assert_eq!(text.as_bytes(), b"1.000000000000000000000000000000000193");
        let mut hex = Text::new();
        render(&mut hex, v, b'a', None, false).unwrap();
        assert_eq!(hex.as_bytes(), b"0x1.0000000000000000000000000001p+0");
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
        let mut text = Text::new();
        render(&mut text, v, b'g', Some(35), false).unwrap();
        assert_eq!(
            text.as_bytes(),
            b"6.4751751194380251109244389582276466e-4966"
        );
    }
}
