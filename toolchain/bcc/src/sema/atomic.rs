//! C11 7.17 and the pinned Clang Language Extensions' __c11_atomic builtins.

use super::*;

pub(crate) const SEQ_CST: u32 = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicOp {
    Init,
    Load,
    Store,
    Exchange,
    CompareExchange,
    Fetch(Binary),
    ThreadFence,
    SignalFence,
    IsLockFree,
    IsLockFreeAt,
}

impl AtomicOp {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        if name == "__builtin_bcc_atomic_is_lock_free" {
            return Some(Self::IsLockFreeAt);
        }
        Some(match name.strip_prefix("__c11_atomic_")? {
            "init" => Self::Init,
            "load" => Self::Load,
            "store" => Self::Store,
            "exchange" => Self::Exchange,
            "compare_exchange_strong" | "compare_exchange_weak" => Self::CompareExchange,
            "fetch_add" => Self::Fetch(Binary::Add),
            "fetch_sub" => Self::Fetch(Binary::Subtract),
            "fetch_and" => Self::Fetch(Binary::BitAnd),
            "fetch_or" => Self::Fetch(Binary::BitOr),
            "fetch_xor" => Self::Fetch(Binary::BitXor),
            "thread_fence" => Self::ThreadFence,
            "signal_fence" => Self::SignalFence,
            "is_lock_free" => Self::IsLockFree,
            _ => return None,
        })
    }
}

impl<'a> FunctionBody<'a, '_> {
    pub(super) fn atomic_builtin(
        &mut self,
        site: &'a Lexeme,
        op: AtomicOp,
        arguments: Vec<parse::Expression<'a>>,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
        use AtomicOp::*;
        let arity = match op {
            ThreadFence | SignalFence | IsLockFree | IsLockFreeAt => 1,
            Init | Load => 2,
            Store | Exchange | Fetch(_) => 3,
            CompareExchange => 5,
        };
        if arguments.len() != arity {
            return Err(site.error(format!("{} requires {arity} arguments", site.text)));
        }
        let mut args = arguments.into_iter();
        let first = self.expression(args.next().unwrap())?;
        let mut typed = Vec::with_capacity(arity);
        let void = Type::new(Kind::Void);
        let boolean = Type::new(Kind::Integer(Integer::Bool));
        let result = match op {
            IsLockFree => {
                typed.push(self.atomic_integer_argument(first, Integer::UnsignedLong)?);
                boolean
            }
            ThreadFence | SignalFence => {
                typed.push(self.atomic_order(first, op, None)?);
                void
            }
            _ => {
                let pointer = first.value_type();
                let Kind::Pointer(object) = &pointer.kind else {
                    return Err(first
                        .site
                        .error("atomic operation requires a pointer to an atomic object"));
                };
                if !object.qualifiers.atomic {
                    return Err(first
                        .site
                        .error("atomic operation requires a pointer to an atomic object"));
                }
                object.check_atomic(first.site)?;
                let layout = object.layout().ok_or_else(|| {
                    first.site.error("atomic operation requires a complete object type")
                })?;
                // WG14 N2244 DR 465, C17 7.17.5.1. The pointed-to type
                // determines the result. Evaluate the pointer for its effects.
                if op == IsLockFreeAt
                    && self.standard == Standard::C17
                    && self.target == Target::X86_64
                {
                    let lock_free = matches!(layout.size, 1 | 2 | 4 | 8)
                        && layout.align >= layout.size
                        && !matches!(object.kind, Kind::Record(_));
                    let result = Expression {
                        site,
                        kind: ExpressionKind::Integer(u64::from(lock_free)),
                        ty: boolean.clone(),
                    };
                    return Ok((
                        ExpressionKind::Comma(Box::new(first), Box::new(result)),
                        boolean,
                    ));
                }
                if !matches!(op, Load | IsLockFreeAt) && object.qualifiers.constant {
                    return Err(first
                        .site
                        .error("atomic operation requires a modifiable atomic object"));
                }
                let value = object.unqualified();
                typed.push(Argument {
                    expr: first,
                    ty: pointer,
                });

                if op == CompareExchange {
                    let expected = self.expression(args.next().unwrap())?;
                    let pointer = value.clone().pointer_to();
                    if !expected.value_type().compatible(&pointer) {
                        return Err(expected.site.error(format!(
                            "atomic compare-exchange expected argument must have type {pointer}"
                        )));
                    }
                    typed.push(Argument {
                        expr: expected,
                        ty: pointer,
                    });
                }
                if !matches!(op, Load | IsLockFreeAt) {
                    let expr = self.expression(args.next().unwrap())?;
                    let ty = match (op, &value.kind) {
                        (Fetch(Binary::Add | Binary::Subtract), Kind::Pointer(element))
                            if element.layout().is_some_and(|layout| layout.size != 0) =>
                        {
                            Type::new(Kind::Integer(Integer::Long))
                        }
                        (Fetch(_), Kind::Integer(integer)) if *integer != Integer::Bool => {
                            value.clone()
                        }
                        (Fetch(_), _) => {
                            return Err(
                                site.error("invalid atomic fetch operation for the object type")
                            );
                        }
                        _ => value.clone(),
                    };
                    if !assignable(&ty, &expr, self.target) {
                        return Err(expr
                            .site
                            .error(format!("cannot convert {} to {ty}", expr.ty)));
                    }
                    typed.push(Argument { expr, ty });
                }
                if let Some(order) = args.next() {
                    let order = self.expression(order)?;
                    typed.push(self.atomic_order(order, op, None)?);
                    if let Some(failure) = args.next() {
                        let success =
                            body::constant_integer(&typed.last().unwrap().expr, self.target)
                                .map(|order| order as u32);
                        let failure = self.expression(failure)?;
                        typed.push(self.atomic_order(failure, op, Some(success))?);
                    }
                }
                match op {
                    Init | Store => void,
                    CompareExchange | IsLockFreeAt => boolean,
                    _ => value,
                }
            }
        };
        Ok((
            ExpressionKind::Atomic {
                op,
                weak: site.is("__c11_atomic_compare_exchange_weak"),
                arguments: typed,
            },
            result,
        ))
    }

    fn atomic_integer_argument(
        &self,
        expr: Expression<'a>,
        integer: Integer,
    ) -> Result<Argument<'a>, Diagnostic> {
        if !matches!(expr.ty.kind, Kind::Integer(_)) {
            return Err(expr
                .site
                .error("atomic builtin argument must be an integer"));
        }
        Ok(Argument {
            expr,
            ty: Type::new(Kind::Integer(integer)),
        })
    }

    fn atomic_order(
        &self,
        expr: Expression<'a>,
        op: AtomicOp,
        success: Option<Option<u32>>,
    ) -> Result<Argument<'a>, Diagnostic> {
        let arg = self.atomic_integer_argument(expr, Integer::Int)?;
        if let Some(value) = body::constant_integer(&arg.expr, self.target) {
            let order = value as u32;
            let valid = order <= SEQ_CST
                && match (op, success) {
                    (AtomicOp::Load, _) => !matches!(order, 3 | 4),
                    (AtomicOp::Store, _) => !matches!(order, 1 | 2 | 4),
                    (AtomicOp::CompareExchange, Some(success)) => {
                        !matches!(order, 3 | 4)
                            && success.is_none_or(|success| {
                                order
                                    <= match success {
                                        3 => 0, // release only
                                        4 => 2, // acquire-release includes acquire
                                        _ => success,
                                    }
                            })
                    }
                    _ => true,
                };
            if !valid {
                return Err(arg
                    .expr
                    .site
                    .error("invalid memory order for this atomic operation"));
            }
        }
        Ok(arg)
    }
}
