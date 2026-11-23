//! Explicit GCC builtins used by Breenix's C headers.

use super::*;
use crate::types::FloatFormat;

// GCC Library Builtins §7.1 and N1570 §§7.12.11.2, 7.24.4.1,
// 7.24.6.1 and 7.24.6.3. Use libc when no inline lowering is needed.
pub(super) fn library_builtin(name: &str) -> Option<(ExpressionKind<'_>, Type)> {
    let int = integer(Integer::Int);
    let size = integer(Integer::UnsignedLong);
    let mut constant_void = Type::new(Kind::Void);
    constant_void.qualifiers.constant = true;
    let const_ptr = constant_void.pointer_to();
    let (name, result, params) = match name {
        "__builtin_memcmp" => ("memcmp", int, vec![const_ptr.clone(), const_ptr, size]),
        "__builtin_memset" => {
            let pointer = Type::new(Kind::Void).pointer_to();
            ("memset", pointer.clone(), vec![pointer, int, size])
        }
        "__builtin_strlen" | "__builtin_nanf" => {
            let mut ch = integer(Integer::Char);
            ch.qualifiers.constant = true;
            if name == "__builtin_strlen" {
                ("strlen", size, vec![ch.pointer_to()])
            } else {
                (
                    "nanf",
                    Type::new(Kind::Float(Float::Single)),
                    vec![ch.pointer_to()],
                )
            }
        }
        _ => return None,
    };
    Some((
        ExpressionKind::Identifier(name),
        Type::new(Kind::Function {
            result: Box::new(result),
            params,
            variadic: false,
            prototype: true,
        }),
    ))
}

pub(super) enum Builtin {
    Infinity(Kind),
    NanF,
    Classify,
    Predicate([i32; 5]),
    Sign,
    Compare(Option<Binary>),
    ByteSwap(Integer),
    Popcount,
    Alloca,
}

impl Builtin {
    pub(super) fn named(name: &str) -> Option<Self> {
        Some(match name {
            "__builtin_inff" | "__builtin_huge_valf" => Self::Infinity(Kind::Float(Float::Single)),
            "__builtin_huge_val" => Self::Infinity(Kind::Float(Float::Double)),
            "__builtin_huge_vall" => Self::Infinity(Kind::LongDouble),
            "__builtin_nanf" => Self::NanF,
            "__builtin_fpclassify" => Self::Classify,
            "__builtin_isnan" => Self::Predicate([1, 0, 0, 0, 0]),
            "__builtin_isinf" => Self::Predicate([0, 1, 0, 0, 0]),
            "__builtin_isfinite" => Self::Predicate([0, 0, 1, 1, 1]),
            "__builtin_isnormal" => Self::Predicate([0, 0, 1, 0, 0]),
            "__builtin_signbit" => Self::Sign,
            "__builtin_isgreater" => Self::Compare(Some(Binary::Greater)),
            "__builtin_isgreaterequal" => Self::Compare(Some(Binary::GreaterEqual)),
            "__builtin_isless" => Self::Compare(Some(Binary::Less)),
            "__builtin_islessequal" => Self::Compare(Some(Binary::LessEqual)),
            "__builtin_islessgreater" => Self::Compare(Some(Binary::NotEqual)),
            "__builtin_isunordered" => Self::Compare(None),
            "__builtin_bswap16" => Self::ByteSwap(Integer::UnsignedShort),
            "__builtin_bswap32" => Self::ByteSwap(Integer::UnsignedInt),
            "__builtin_bswap64" => Self::ByteSwap(Integer::UnsignedLong),
            "__builtin_popcountl" => Self::Popcount,
            "__builtin_alloca" => Self::Alloca,
            _ => return None,
        })
    }

    fn arity(&self) -> usize {
        match self {
            Self::Infinity(_) => 0,
            Self::Compare(_) => 2,
            Self::Classify => 6,
            _ => 1,
        }
    }
}

fn integer(width: Integer) -> Type {
    Type::new(Kind::Integer(width))
}

fn float_value(ty: &Type) -> bool {
    matches!(ty.kind, Kind::Float(_) | Kind::LongDouble)
}

// GCC Floating-Point Format Builtins §7.2.1, with N1570 §7.22.1.4
// for the base prefixes. Only the low payload bits are kept.
fn nan_payload(bytes: &[u8]) -> Option<u128> {
    let tag = bytes.split(|&byte| byte == 0).next()?;
    let tag = std::str::from_utf8(tag)
        .ok()?
        .trim_start_matches(|c: char| c.is_ascii_whitespace());
    if tag.is_empty() {
        return Some(0);
    }
    let negative = tag.starts_with('-');
    let number = tag.strip_prefix(['+', '-']).unwrap_or(tag);
    let (radix, digits) = if let Some(hex) = number
        .strip_prefix("0x")
        .or_else(|| number.strip_prefix("0X"))
    {
        (16, hex)
    } else if number.starts_with('0') {
        (8, number)
    } else {
        (10, number)
    };
    if digits.is_empty() {
        return None;
    }
    let magnitude = digits
        .chars()
        .try_fold(0_u64, |value, ch| {
            Some(
                value
                    .saturating_mul(radix.into())
                    .saturating_add(ch.to_digit(radix)?.into()),
            )
        })?
        .min(i64::MAX as u64 + u64::from(negative));
    Some(u128::from(if negative {
        magnitude.wrapping_neg()
    } else {
        magnitude
    }))
}

impl<'a> FunctionBody<'a, '_> {
    pub(super) fn builtin(
        &mut self,
        site: &'a Lexeme,
        builtin: Builtin,
        arguments: Vec<parse::Expression<'a>>,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
        if arguments.len() != builtin.arity() {
            return Err(site.error(format!(
                "{} requires {} arguments",
                site.text,
                builtin.arity()
            )));
        }
        let mut arguments = arguments
            .into_iter()
            .map(|arg| self.expression(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let int = integer(Integer::Int);
        match builtin {
            Builtin::Infinity(kind) => {
                let bits = FloatFormat::of(&kind, self.target).infinity();
                Ok((ExpressionKind::Float(bits), Type::new(kind)))
            }
            Builtin::NanF => {
                let arg = arguments.pop().unwrap();
                let ty = Type::new(Kind::Float(Float::Single));
                if let ExpressionKind::String(bytes) = &arg.kind {
                    if matches!(&arg.ty.kind, Kind::Array(element, _) if element.kind == Kind::Integer(Integer::Char))
                    {
                        if let Some(payload) = nan_payload(bytes) {
                            let bits = FloatFormat::of(&ty.kind, self.target).quiet_nan(payload);
                            return Ok((ExpressionKind::Float(bits), ty));
                        }
                    }
                }
                let (kind, function) = library_builtin("__builtin_nanf").unwrap();
                let Kind::Function { params, .. } = &function.kind else {
                    unreachable!()
                };
                let param = params[0].clone();
                if !assignable(&param, &arg, self.target) {
                    return Err(arg
                        .site
                        .error("__builtin_nanf requires a character pointer"));
                }
                Ok((
                    ExpressionKind::Call {
                        callee: Box::new(Expression {
                            site,
                            kind,
                            ty: function,
                        }),
                        arguments: vec![Argument {
                            expr: arg,
                            ty: param,
                        }],
                    },
                    ty,
                ))
            }
            Builtin::Classify | Builtin::Predicate(_) | Builtin::Sign => {
                let operand = Box::new(arguments.pop().unwrap());
                if !float_value(&operand.ty) {
                    return Err(operand.site.error("expected a floating-point argument"));
                }
                let kind = match builtin {
                    Builtin::Classify => {
                        let mut values = [0; 5];
                        for (value, arg) in values.iter_mut().zip(arguments) {
                            *value = body::constant_integer(&arg, self.target).ok_or_else(|| {
                                arg.site.error("expected a constant FP classification code")
                            })? as i32;
                        }
                        ExpressionKind::FloatClass { operand, values }
                    }
                    Builtin::Predicate(values) => ExpressionKind::FloatClass { operand, values },
                    Builtin::Sign => ExpressionKind::SignBit(operand),
                    _ => unreachable!(),
                };
                Ok((kind, int))
            }
            Builtin::Compare(op) => {
                let right = arguments.pop().unwrap();
                let left = arguments.pop().unwrap();
                self.quiet_compare(site, left, right, op)
            }
            Builtin::Popcount | Builtin::ByteSwap(_) => {
                let argument = arguments.pop().unwrap();
                self.swap_or_count(site, argument, builtin)
            }
            Builtin::Alloca => {
                if self.static_literals.is_some() {
                    return Err(site.error("__builtin_alloca requires block scope"));
                }
                let size = integer(Integer::UnsignedLong);
                let operand = arguments.pop().unwrap();
                if !assignable(&size, &operand, self.target) {
                    return Err(operand
                        .site
                        .error("__builtin_alloca requires a size argument"));
                }
                self.has_alloca = true;
                Ok((
                    ExpressionKind::Alloca(Box::new(operand)),
                    Type::new(Kind::Void).pointer_to(),
                ))
            }
        }
    }

    fn temporary(&mut self, site: &'a Lexeme, ty: &Type) -> usize {
        let local = self.locals.len();
        self.locals.push(Local {
            object: Object {
                name: "",
                site: &site.site,
                ty: ty.clone(),
                alignment: None,
            },
            storage: LocalStorage::Automatic,
        });
        local
    }

    // Test the original formats for NaNs before performing any usual
    // arithmetic conversions or comparisons (N1570 §7.12.14).
    fn quiet_compare(
        &mut self,
        site: &'a Lexeme,
        left: Expression<'a>,
        right: Expression<'a>,
        op: Option<Binary>,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
        let fallback = Binary::NotEqual;
        let Some((left_type, right_type, int)) =
            binary_types(op.unwrap_or(fallback), &left, &right, self.target)
                .filter(|(left, _, _)| float_value(left))
        else {
            return Err(
                site.error("comparison requires arithmetic operands with a floating-point type")
            );
        };
        let left_temp = self.temporary(site, &left.value_type());
        let right_temp = self.temporary(site, &right.value_type());
        let temp = |local, ty: Type| Expression {
            site,
            ty,
            kind: ExpressionKind::Local(local),
        };
        let assign = |local, right: Expression<'a>| Expression {
            site,
            ty: right.value_type(),
            kind: ExpressionKind::Assignment {
                left: Box::new(temp(local, right.value_type())),
                right: Box::new(right),
                operation: None,
            },
        };
        let left_ty = left.value_type();
        let right_ty = right.value_type();
        let nan = |local, ty: &Type| Expression {
            site,
            ty: int.clone(),
            kind: if float_value(ty) {
                ExpressionKind::FloatClass {
                    operand: Box::new(temp(local, ty.clone())),
                    values: [1, 0, 0, 0, 0],
                }
            } else {
                ExpressionKind::Integer(0)
            },
        };
        let binary = |op, left, right, left_type, right_type| Expression {
            site,
            ty: int.clone(),
            kind: ExpressionKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                left_type,
                right_type,
            },
        };
        let unordered = binary(
            Binary::LogicalOr,
            nan(left_temp, &left_ty),
            nan(right_temp, &right_ty),
            int.clone(),
            int.clone(),
        );
        let result = if let Some(op) = op {
            Expression {
                site,
                ty: int.clone(),
                kind: ExpressionKind::Conditional {
                    condition: Box::new(unordered),
                    yes: Box::new(Expression {
                        site,
                        ty: int.clone(),
                        kind: ExpressionKind::Integer(0),
                    }),
                    no: Box::new(binary(
                        op,
                        temp(left_temp, left_ty),
                        temp(right_temp, right_ty),
                        left_type,
                        right_type,
                    )),
                },
            }
        } else {
            unordered
        };
        let then = Expression {
            site,
            ty: int.clone(),
            kind: ExpressionKind::Comma(Box::new(assign(right_temp, right)), Box::new(result)),
        };
        Ok((
            ExpressionKind::Comma(Box::new(assign(left_temp, left)), Box::new(then)),
            int,
        ))
    }

    fn swap_or_count(
        &mut self,
        site: &'a Lexeme,
        argument: Expression<'a>,
        builtin: Builtin,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
        // GCC Bit Operation Builtins §7.2.2 and Byte-Swapping Builtins §7.2.3.
        let word = integer(Integer::UnsignedLong);
        let (width, shifts, op) = match builtin {
            Builtin::ByteSwap(value) => (
                value,
                &[8, 16, 32][..value.layout().size.trailing_zeros() as usize],
                Binary::BitOr,
            ),
            Builtin::Popcount => (
                Integer::UnsignedLong,
                &[1, 2, 4, 8, 16, 32][..],
                Binary::Add,
            ),
            _ => unreachable!(),
        };
        let input = integer(width);
        let result = if matches!(builtin, Builtin::Popcount) {
            integer(Integer::Int)
        } else {
            input.clone()
        };
        if !assignable(&input, &argument, self.target) {
            return Err(argument
                .site
                .error(format!("{} requires an arithmetic argument", site.text)));
        }
        if let Some(bits) = body::constant_integer(&argument, self.target) {
            let value = match builtin {
                Builtin::ByteSwap(Integer::UnsignedShort) => u64::from((bits as u16).swap_bytes()),
                Builtin::ByteSwap(Integer::UnsignedInt) => u64::from((bits as u32).swap_bytes()),
                Builtin::ByteSwap(_) => bits.swap_bytes(),
                Builtin::Popcount => bits.count_ones().into(),
                _ => unreachable!(),
            };
            return Ok((ExpressionKind::Integer(value), result));
        }

        // Evaluate once. Each stage swaps or sums the adjacent groups of bits.
        let local = self.temporary(site, &word);
        let expr = |kind| Expression {
            site,
            ty: word.clone(),
            kind,
        };
        let temp = || expr(ExpressionKind::Local(local));
        let literal = |bits| expr(ExpressionKind::Integer(bits));
        let binary = |op, left, right| {
            expr(ExpressionKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                left_type: word.clone(),
                right_type: word.clone(),
            })
        };
        let assign = |right| {
            expr(ExpressionKind::Assignment {
                left: Box::new(temp()),
                right: Box::new(right),
                operation: None,
            })
        };
        let comma = |left, right| expr(ExpressionKind::Comma(Box::new(left), Box::new(right)));
        let argument = if matches!(builtin, Builtin::Popcount) {
            argument
        } else {
            Expression {
                site,
                ty: input,
                kind: ExpressionKind::Cast(Box::new(argument)),
            }
        };
        let mut value = assign(argument);
        for &shift in shifts {
            let mask = u64::MAX / ((1u64 << shift) + 1);
            let low = binary(Binary::BitAnd, temp(), literal(mask));
            let shifted = binary(Binary::ShiftRight, temp(), literal(shift));
            let high = binary(Binary::BitAnd, shifted, literal(mask));
            let low = if op == Binary::BitOr {
                binary(Binary::ShiftLeft, low, literal(shift))
            } else {
                low
            };
            value = comma(value, assign(binary(op, low, high)));
        }
        Ok((ExpressionKind::Cast(Box::new(comma(value, temp()))), result))
    }
}
