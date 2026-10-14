//! Variadic argument decoding. Long double is read as bits using the platform ABI.
//! ABI sources: Arm AAPCS64 appendix 14, AMD64 psABI 3.5.7; no libc implementation.
use super::format::{Format, Length};
use core::ffi::VaList;
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Missing,
    I32,
    U32,
    I64,
    U64,
    Pointer,
    Double,
    LongDouble,
}
#[derive(Clone, Copy)]
pub(super) struct Binary {
    pub negative: bool,
    pub significand: u128,
    pub exponent: i32,
    pub fraction_bits: u32,
    pub special: u8,
}
impl Binary {
    pub fn double(v: f64) -> Self {
        let b = v.to_bits();
        let e = ((b >> 52) & 2047) as i32;
        let frac = b & ((1u64 << 52) - 1);
        Self {
            negative: b >> 63 != 0,
            significand: if e == 0 {
                frac as u128
            } else {
                ((1u64 << 52) | frac) as u128
            },
            exponent: if e == 0 { -1074 } else { e - 1023 - 52 },
            fraction_bits: 52,
            special: if e == 2047 {
                if frac == 0 {
                    1
                } else {
                    2
                }
            } else {
                0
            },
        }
    }
    #[cfg(all(target_arch = "aarch64", not(target_os = "macos")))]
    fn quad(b: u128) -> Self {
        let e = ((b >> 112) & 32767) as i32;
        let frac = b & ((1u128 << 112) - 1);
        Self {
            negative: b >> 127 != 0,
            significand: if e == 0 { frac } else { (1u128 << 112) | frac },
            exponent: if e == 0 { -16494 } else { e - 16383 - 112 },
            fraction_bits: 112,
            special: if e == 32767 {
                if frac == 0 {
                    1
                } else {
                    2
                }
            } else {
                0
            },
        }
    }
    #[cfg(target_arch = "x86_64")]
    fn extended(b: u128) -> Self {
        let e = ((b >> 64) & 32767) as i32;
        let sig = b as u64;
        Self {
            negative: (b >> 79) & 1 != 0,
            significand: sig as u128,
            exponent: if e == 0 { -16445 } else { e - 16383 - 63 },
            fraction_bits: 63,
            special: if e == 32767 {
                if sig == 1u64 << 63 {
                    1
                } else {
                    2
                }
            } else {
                0
            },
        }
    }
}
unsafe fn long_double(ap: &mut VaList<'_, '_>) -> Binary {
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        Binary::double(ap.arg::<f64>())
    }
    #[cfg(all(target_arch = "aarch64", not(target_os = "macos")))]
    {
        #[repr(C)]
        struct List {
            stack: *mut u8,
            gr_top: *mut u8,
            vr_top: *mut u8,
            gr_offs: i32,
            vr_offs: i32,
        }
        let list = &mut *(&mut **ap as *mut _ as *mut List);
        let p = if list.vr_offs < 0 {
            let offset = list.vr_offs;
            list.vr_offs += 16;
            list.vr_top.offset(offset as isize)
        } else {
            let p = ((list.stack as usize + 15) & !15) as *mut u8;
            list.stack = p.add(16);
            p
        };
        Binary::quad(core::ptr::read_unaligned(p as *const u128))
    }
    #[cfg(target_arch = "x86_64")]
    {
        #[repr(C)]
        struct List {
            gp_offset: i32,
            fp_offset: i32,
            overflow: *mut u8,
            reg_save: *mut u8,
        }
        let list = &mut *(&mut **ap as *mut _ as *mut List);
        let p = ((list.overflow as usize + 15) & !15) as *mut u8;
        list.overflow = p.add(16);
        Binary::extended(core::ptr::read_unaligned(p as *const u128))
    }
}
#[derive(Clone, Copy)]
enum Value {
    Integer(u64),
    Pointer(*mut u8),
    Float(Binary),
}
unsafe fn read(ap: &mut VaList<'_, '_>, kind: Kind) -> Value {
    match kind {
        Kind::I32 => Value::Integer(ap.arg::<i32>() as i64 as u64),
        Kind::U32 => Value::Integer(ap.arg::<u32>() as u64),
        Kind::I64 => Value::Integer(ap.arg::<i64>() as u64),
        Kind::U64 => Value::Integer(ap.arg::<u64>()),
        Kind::Pointer => Value::Pointer(ap.arg::<*mut u8>()),
        Kind::Double => Value::Float(Binary::double(ap.arg::<f64>())),
        Kind::LongDouble => Value::Float(long_double(ap)),
        Kind::Missing => unreachable!(),
    }
}
#[derive(Clone, Copy)]
pub(super) struct Spec {
    pub left: bool,
    pub plus: bool,
    pub space: bool,
    pub alternate: bool,
    pub zero: bool,
    pub width: usize,
    pub precision: Option<usize>,
    pub argument: usize,
    pub width_argument: Option<usize>,
    pub precision_argument: Option<usize>,
    pub length: Length,
    pub conversion: u8,
}
unsafe fn number(fmt: &Format, i: &mut usize) -> Result<usize, i32> {
    let mut n = 0usize;
    while (48..=57).contains(&fmt.at(*i)) {
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add((fmt.at(*i) - 48) as usize))
            .filter(|&n| n <= i32::MAX as usize)
            .ok_or(75)?;
        *i += 1;
    }
    Ok(n)
}
unsafe fn position(fmt: &Format, i: &mut usize) -> Result<usize, i32> {
    let save = *i;
    let n = number(fmt, i)?;
    if fmt.at(*i) == 36 {
        *i += 1;
        if n == 0 {
            Err(22)
        } else {
            Ok(n)
        }
    } else {
        *i = save;
        Ok(0)
    }
}
impl Spec {
    pub unsafe fn parse(fmt: &Format, i: &mut usize) -> Result<Self, i32> {
        let mut s = Self {
            left: false,
            plus: false,
            space: false,
            alternate: false,
            zero: false,
            width: 0,
            precision: None,
            argument: position(fmt, i)?,
            width_argument: None,
            precision_argument: None,
            length: Length::None,
            conversion: 0,
        };
        loop {
            match fmt.at(*i) {
                45 => s.left = true,
                43 => s.plus = true,
                32 => s.space = true,
                35 => s.alternate = true,
                48 => s.zero = true,
                _ => break,
            }
            *i += 1;
        }
        if fmt.at(*i) == 42 {
            *i += 1;
            s.width_argument = Some(position(fmt, i)?);
        } else {
            s.width = number(fmt, i)?;
        }
        if fmt.at(*i) == 46 {
            *i += 1;
            if fmt.at(*i) == 42 {
                *i += 1;
                s.precision_argument = Some(position(fmt, i)?);
            } else {
                s.precision = Some(number(fmt, i)?);
            }
        }
        let c = fmt.at(*i);
        s.length = match c {
            104 => Length::H,
            108 => Length::L,
            106 => Length::J,
            122 => Length::Z,
            116 => Length::T,
            76 => Length::BigL,
            _ => Length::None,
        };
        if s.length != Length::None {
            *i += 1;
            if c == 104 && fmt.at(*i) == 104 {
                s.length = Length::Hh;
                *i += 1;
            } else if c == 108 && fmt.at(*i) == 108 {
                s.length = Length::Ll;
                *i += 1;
            }
        }
        s.conversion = fmt.at(*i) as u8;
        if s.conversion == 0 {
            return Err(22);
        }
        *i += 1;
        Ok(s)
    }
    fn kind(&self) -> Result<Kind, i32> {
        let wide = matches!(
            self.length,
            Length::L | Length::Ll | Length::J | Length::Z | Length::T
        );
        match self.conversion {
            b'd' | b'i' => Ok(if wide { Kind::I64 } else { Kind::I32 }),
            b'u' | b'o' | b'x' | b'X' => Ok(if wide { Kind::U64 } else { Kind::U32 }),
            b'p' | b's' | b'n' => Ok(Kind::Pointer),
            b'c' => Ok(Kind::I32),
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                Ok(if self.length == Length::BigL {
                    Kind::LongDouble
                } else {
                    Kind::Double
                })
            }
            _ => Err(22),
        }
    }
}
// POSIX NL_ARGMAX is a published implementation limit; 4096 exceeds its minimum.
const LIMIT: usize = 4096;
pub(super) struct Arguments<'a, 'f> {
    ap: VaList<'a, 'f>,
    kinds: [Kind; LIMIT],
    positional: bool,
}
impl<'a, 'f> Arguments<'a, 'f> {
    pub unsafe fn new(ap: VaList<'a, 'f>, fmt: &Format) -> Result<Self, i32> {
        let mut result = Self {
            ap,
            kinds: [Kind::Missing; LIMIT],
            positional: false,
        };
        let mut sequential = false;
        let mut maximum = 0;
        let mut i = 0;
        while fmt.at(i) != 0 {
            if fmt.at(i) != 37 {
                i += 1;
                continue;
            }
            i += 1;
            if fmt.at(i) == 37 {
                i += 1;
                continue;
            }
            let spec = Spec::parse(fmt, &mut i)?;
            for (position, kind) in [
                (spec.width_argument, Kind::I32),
                (spec.precision_argument, Kind::I32),
                (Some(spec.argument), spec.kind()?),
            ] {
                if let Some(pos) = position {
                    if pos == 0 {
                        sequential = true;
                        continue;
                    }
                    if pos > LIMIT {
                        return Err(22);
                    }
                    result.positional = true;
                    maximum = maximum.max(pos);
                    if result.kinds[pos - 1] != Kind::Missing && result.kinds[pos - 1] != kind {
                        return Err(22);
                    }
                    result.kinds[pos - 1] = kind;
                }
            }
        }
        if result.positional && (sequential || result.kinds[..maximum].contains(&Kind::Missing)) {
            return Err(22);
        }
        Ok(result)
    }
    unsafe fn value(&mut self, pos: usize, kind: Kind) -> Value {
        if !self.positional {
            return read(&mut self.ap, kind);
        }
        let mut copy = (*self.ap).clone();
        let mut ap = copy.as_va_list();
        for &k in &self.kinds[..pos - 1] {
            read(&mut ap, k);
        }
        read(&mut ap, kind)
    }
    pub unsafe fn widths(&mut self, s: &mut Spec) {
        if let Some(pos) = s.width_argument {
            let Value::Integer(v) = self.value(pos, Kind::I32) else {
                unreachable!()
            };
            let v = v as i32;
            if v < 0 {
                s.left = true;
                s.width = v.unsigned_abs() as usize;
            } else {
                s.width = v as usize;
            }
        }
        if let Some(pos) = s.precision_argument {
            let Value::Integer(v) = self.value(pos, Kind::I32) else {
                unreachable!()
            };
            let v = v as i32;
            s.precision = if v < 0 { None } else { Some(v as usize) };
        }
    }
    pub unsafe fn integer(&mut self, s: &Spec) -> u64 {
        let Value::Integer(v) = self.value(s.argument, s.kind().unwrap()) else {
            unreachable!()
        };
        match s.length {
            Length::Hh => {
                if s.conversion == b'd' || s.conversion == b'i' {
                    v as i8 as i64 as u64
                } else {
                    v as u8 as u64
                }
            }
            Length::H => {
                if s.conversion == b'd' || s.conversion == b'i' {
                    v as i16 as i64 as u64
                } else {
                    v as u16 as u64
                }
            }
            _ => v,
        }
    }
    pub unsafe fn pointer(&mut self, s: &Spec) -> *mut u8 {
        let Value::Pointer(v) = self.value(s.argument, Kind::Pointer) else {
            unreachable!()
        };
        v
    }
    pub unsafe fn floating(&mut self, s: &Spec) -> Binary {
        let Value::Float(v) = self.value(s.argument, s.kind().unwrap()) else {
            unreachable!()
        };
        v
    }
}
