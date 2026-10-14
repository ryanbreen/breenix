//! Binary64 formatting using exact decimal integer arithmetic, without libc/libm code.
//! x = significand * 2^e; for negative e, x = significand * 5^(-e) / 10^(-e).
//! At most 1074 fractional places and 309 integer places are required.
use super::format::Text;

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
    d: [u8; 1200],
    n: usize,
    e: i32,
}
impl Decimal {
    fn new(bits: u64) -> Self {
        let exponent = ((bits >> 52) & 2047) as i32;
        let mut sig = bits & ((1u64 << 52) - 1);
        let binary = if exponent == 0 {
            -1074
        } else {
            sig |= 1u64 << 52;
            exponent - 1023 - 52
        };
        let mut s = Self {
            d: [0; 1200],
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
        if bits & 0x7fffffffffffffff == 0 {
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
    value: f64,
    conversion: u8,
    precision: Option<usize>,
    alternate: bool,
) -> Result<(), i32> {
    let bits = value.to_bits();
    let neg = bits >> 63 != 0;
    let abs = bits & 0x7fffffffffffffff;
    let upper = conversion.is_ascii_uppercase();
    let c = conversion.to_ascii_lowercase();
    if abs >= 0x7ff0000000000000 {
        return out.bytes(if abs == 0x7ff0000000000000 {
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
        let mut fraction = abs & ((1u64 << 52) - 1);
        let raw = (abs >> 52) as i32;
        let mut lead = if raw == 0 { 0 } else { 1 };
        let e = if abs == 0 {
            0
        } else if raw == 0 {
            -1022
        } else {
            raw - 1023
        };
        let p = precision.unwrap_or_else(|| {
            let mut p = 13;
            while p > 0 && ((fraction >> ((13 - p) * 4)) & 15) == 0 {
                p -= 1;
            }
            p
        });
        if p < 13 {
            let shift = (13 - p) * 4;
            let mask = (1u64 << shift) - 1;
            let rest = fraction & mask;
            let halfway = 1u64 << (shift - 1);
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
                fraction += 1u64 << shift;
                if fraction >> 52 != 0 {
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
            out.byte(if i < 13 {
                alphabet[((fraction >> ((12 - i) * 4)) & 15) as usize]
            } else {
                b'0'
            })?
        }
        return exponent(out, e, if upper { b'P' } else { b'p' }, 1);
    }
    let mut d = Decimal::new(abs);
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
            while p > 0 {
                let index = if scientific {
                    p as i32
                } else {
                    d.e + 1 + p as i32 - 1
                };
                if d.digit(index) != b'0' {
                    break;
                }
                p -= 1;
            }
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
