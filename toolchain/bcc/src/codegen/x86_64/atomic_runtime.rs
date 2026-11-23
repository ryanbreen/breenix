//! Generic size-first calls from LLVM 22.1.0, “Libcalls: __atomic_*”.
//! brt and inline operations share the AMD64 size and alignment decision.

use super::atomic::Width;
use super::*;
use crate::sema::SEQ_CST;

struct AtomicUpdate<'a, 'src> {
    place: &'a Expression<'src>,
    object: &'a Type,
    lhs_type: &'a Type,
    rhs_type: &'a Type,
    rhs: Option<&'a Expression<'src>>,
    order: Option<&'a Argument<'src>>,
    op: Binary,
    return_old: bool,
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn atomic_pointer_alignment(pointer: &Expression<'src>) -> usize {
        match &pointer.kind {
            ExpressionKind::Unary(Unary::Address, object) => Self::atomic_lvalue_alignment(object),
            _ => 1,
        }
    }

    pub(super) fn atomic_lvalue_alignment(object: &Expression<'src>) -> usize {
        match &object.kind {
            ExpressionKind::Identifier(_)
            | ExpressionKind::Local(_)
            | ExpressionKind::ThreadLocal(_)
            | ExpressionKind::CompoundLiteral { .. }
            | ExpressionKind::StaticLiteral(_) => {
                object.ty.layout().map_or(1, |layout| layout.align)
            }
            ExpressionKind::Member { base, offset, .. } => {
                let alignment = Self::atomic_lvalue_alignment(base);
                if offset % alignment == 0 {
                    alignment
                } else {
                    1 << offset.trailing_zeros()
                }
            }
            ExpressionKind::Unary(Unary::Dereference, pointer) => {
                Self::atomic_pointer_alignment(pointer)
            }
            _ => 1,
        }
    }

    pub(super) fn atomic_needs_storage(object: &Expression<'src>) -> bool {
        if !object.ty.qualifiers.atomic {
            return false;
        }
        object.ty.layout().is_none_or(|layout| {
            Width::of(layout.size).is_none()
                || Self::atomic_lvalue_alignment(object) < layout.size
                || matches!(object.ty.kind, Kind::Record(_))
        })
    }

    pub(super) fn atomic_is_lock_free_at(
        &mut self,
        arg: &Argument<'src>,
    ) -> Result<(), Diagnostic> {
        let Kind::Pointer(object) = &arg.ty.kind else {
            unreachable!("sema checks atomic pointer arguments");
        };
        let size = object.layout().unwrap().size;
        self.value(&arg.expr)?;
        if Width::of(size).is_none() {
            self.text.push_str("    xor eax, eax\n");
        } else if Self::atomic_pointer_alignment(&arg.expr) >= size {
            self.text.push_str("    mov eax, 1\n");
        } else {
            writeln!(
                self.text,
                "    test rax, {}\n    sete al\n    movzx eax, al",
                size - 1
            )
            .unwrap();
        }
        Ok(())
    }

    // [rsp] saves the object address across nested expressions and calls.
    fn atomic_access(
        &mut self,
        size: usize,
        alignment: usize,
        mut emit: impl FnMut(&mut Self, Option<Width>),
    ) {
        let Some(width) = Width::of(size) else {
            emit(self, None);
            return;
        };
        self.text.push_str("    mov r11, qword [rsp]\n");
        if alignment >= size {
            emit(self, Some(width));
            return;
        }
        let slow = self.label();
        let done = self.label();
        writeln!(self.text, "    test r11, {}\n    jne {slow}", size - 1).unwrap();
        emit(self, Some(width));
        writeln!(self.text, "    jmp {done}\n{slow}:").unwrap();
        emit(self, None);
        writeln!(self.text, "{done}:").unwrap();
    }

    fn brt_call(&mut self, symbol: &'static str, size: usize) {
        self.externs.insert(symbol);
        writeln!(
            self.text,
            "    mov rdi, {size}\n    mov rsi, qword [rsp]\n    call {symbol}@plt"
        )
        .unwrap();
    }

    fn atomic_buffer(&mut self, object: &Type, site: &Position) -> Result<String, Diagnostic> {
        let layout = object.layout().ok_or_else(|| {
            Diagnostic::new(site, "atomic access requires a complete object type")
        })?;
        Ok(self.frame_slot(layout, site)?.memory(0))
    }

    fn read_atomic_buffer(&mut self, object: &Type, buffer: &str) {
        if matches!(object.kind, Kind::Record(_)) {
            writeln!(self.text, "    lea rax, {buffer}").unwrap();
        } else {
            self.load(object, buffer);
        }
    }

    fn write_atomic_buffer(&mut self, object: &Type, buffer: &str) {
        if object.kind == Kind::LongDouble {
            // FSTP writes ten bytes of the psABI's sixteen-byte object.
            writeln!(
                self.text,
                "    lea r10, {buffer}\n    mov qword [r10 + 8], 0"
            )
            .unwrap();
        }
        self.store_object(object, buffer);
    }

    pub(super) fn atomic_load_with_storage(
        &mut self,
        object: &Type,
        alignment: usize,
        site: &Position,
    ) -> Result<(), Diagnostic> {
        let result = self.atomic_buffer(object, site)?;
        self.adjust_stack("sub", 16);
        self.text.push_str("    mov qword [rsp], rax\n");
        self.atomic_access(
            object.layout().unwrap().size,
            alignment,
            |emitter, width| {
                if let Some(width) = width {
                    emitter.atomic_inline(AtomicOp::Load, object, width, Some(true));
                    emitter.write_atomic_inline_result(object, width, &result);
                } else {
                    writeln!(
                        emitter.text,
                        "    lea rdx, {result}\n    mov ecx, {SEQ_CST}"
                    )
                    .unwrap();
                    emitter.brt_call("__atomic_load", object.layout().unwrap().size);
                    emitter.read_atomic_buffer(object, &result);
                }
            },
        );
        self.adjust_stack("add", 16);
        Ok(())
    }

    pub(super) fn atomic_store_with_storage(
        &mut self,
        left: &Expression<'src>,
        right: &Expression<'src>,
    ) -> Result<(), Diagnostic> {
        let object = &left.ty;
        let size = object.layout().unwrap().size;
        let value = self.atomic_buffer(object, left.position())?;
        self.address(left)?;
        self.adjust_stack("sub", 16);
        self.text.push_str("    mov qword [rsp], rax\n");
        self.converted(right, object)?;
        self.write_atomic_buffer(object, &value);
        self.atomic_access(
            size,
            Self::atomic_lvalue_alignment(left),
            |emitter, width| {
                if let Some(width) = width {
                    emitter.stage_atomic_inline_value(width, &value, 8);
                    emitter.atomic_inline(AtomicOp::Store, object, width, Some(true));
                } else {
                    writeln!(emitter.text, "    lea rdx, {value}\n    mov ecx, {SEQ_CST}").unwrap();
                    emitter.brt_call("__atomic_store", size);
                }
            },
        );
        self.read_atomic_buffer(object, &value);
        self.adjust_stack("add", 16);
        Ok(())
    }

    fn stage_atomic_inline_value(&mut self, width: Width, value: &str, offset: usize) {
        let Width {
            memory,
            accumulator,
            ..
        } = width;
        writeln!(
            self.text,
            "    mov {accumulator}, {memory} ptr {value}\n    mov qword [rsp + {offset}], rax"
        )
        .unwrap();
    }

    fn write_atomic_inline_result(&mut self, object: &Type, width: Width, result: &str) {
        if matches!(object.kind, Kind::Record(_)) {
            let Width {
                memory,
                accumulator,
                ..
            } = width;
            writeln!(
                self.text,
                "    mov {memory} ptr {result}, {accumulator}\n    lea rax, {result}"
            )
            .unwrap();
        }
    }

    pub(super) fn atomic_builtin_with_storage(
        &mut self,
        op: AtomicOp,
        object: &Type,
        alignment: usize,
        args: &[Argument<'src>],
    ) -> Result<(), Diagnostic> {
        use AtomicOp::*;
        let size = object.layout().unwrap().size;
        let site = args[0].expr.position();
        if let Fetch(binary) = op {
            let rhs = &args[1];
            return self.atomic_update_with_storage(AtomicUpdate {
                place: &args[0].expr,
                object,
                lhs_type: object,
                rhs_type: &rhs.ty,
                rhs: Some(&rhs.expr),
                order: Some(&args[2]),
                op: binary,
                return_old: true,
            });
        }

        self.value(&args[0].expr)?;
        self.adjust_stack("sub", 48);
        self.text.push_str("    mov qword [rsp], rax\n");

        if op == CompareExchange {
            self.value(&args[1].expr)?;
            self.text.push_str("    mov qword [rsp + 8], rax\n");
        }
        let mut index = if op == CompareExchange { 2 } else { 1 };
        let value = if op == Load {
            None
        } else {
            let buffer = self.atomic_buffer(object, site)?;
            self.converted(&args[index].expr, object)?;
            self.write_atomic_buffer(object, &buffer);
            index += 1;
            Some(buffer)
        };
        let result = if matches!(op, Load | Exchange) {
            Some(self.atomic_buffer(object, site)?)
        } else {
            None
        };

        // Spill orders after evaluating the object and value exactly once.
        if op == Init {
            self.text.push_str("    mov dword [rsp + 24], 0\n");
        }
        for (index, order) in args[index..].iter().enumerate() {
            self.converted(&order.expr, &order.ty)?;
            writeln!(self.text, "    mov dword [rsp + {}], eax", 24 + index * 8).unwrap();
        }

        self.atomic_access(size, alignment, |emitter, width| {
            if let Some(width) = width {
                if let Some(value) = &value {
                    let offset = if op == CompareExchange { 16 } else { 8 };
                    emitter.stage_atomic_inline_value(width, value, offset);
                }
                emitter.text.push_str("    mov eax, dword [rsp + 24]\n");
                emitter.atomic_inline(op, object, width, None);
                if let Some(result) = &result {
                    emitter.write_atomic_inline_result(object, width, result);
                }
            } else {
                let symbol = match op {
                    Load => {
                        let result = result.as_ref().unwrap();
                        writeln!(emitter.text, "    lea rdx, {result}\n    mov ecx, dword [rsp + 24]").unwrap();
                        "__atomic_load"
                    }
                    Init | Store => {
                        let value = value.as_ref().unwrap();
                        writeln!(emitter.text, "    lea rdx, {value}\n    mov ecx, dword [rsp + 24]").unwrap();
                        "__atomic_store"
                    }
                    Exchange => {
                        let value = value.as_ref().unwrap();
                        let result = result.as_ref().unwrap();
                        writeln!(emitter.text, "    lea rdx, {value}\n    lea rcx, {result}\n    mov r8d, dword [rsp + 24]").unwrap();
                        "__atomic_exchange"
                    }
                    CompareExchange => {
                        let value = value.as_ref().unwrap();
                        writeln!(emitter.text, "    mov rdx, qword [rsp + 8]\n    lea rcx, {value}\n    mov r8d, dword [rsp + 24]\n    mov r9d, dword [rsp + 32]").unwrap();
                        "__atomic_compare_exchange"
                    }
                    _ => unreachable!(),
                };
                emitter.brt_call(symbol, size);
                if let Some(result) = &result {
                    emitter.read_atomic_buffer(object, result);
                } else if op == CompareExchange {
                    emitter.text.push_str("    movzx eax, al\n");
                }
            }
        });
        self.adjust_stack("add", 48);
        Ok(())
    }

    pub(super) fn atomic_rmw(
        &mut self,
        object: &Expression<'src>,
        rhs: Option<&Expression<'src>>,
        op: Binary,
        (lhs_type, rhs_type): (&Type, &Type),
        postfix: bool,
    ) -> Result<(), Diagnostic> {
        self.atomic_update_with_storage(AtomicUpdate {
            place: object,
            object: &object.ty,
            lhs_type,
            rhs_type,
            rhs,
            order: None,
            op,
            return_old: postfix,
        })
    }

    // The CAS loop shares evaluation, conversions and fenv handling across
    // inline and locked paths. The runtime updates expected on each failure.
    fn atomic_update_with_storage(
        &mut self,
        update: AtomicUpdate<'_, 'src>,
    ) -> Result<(), Diagnostic> {
        let AtomicUpdate {
            place,
            object,
            lhs_type,
            rhs_type,
            rhs,
            order,
            op,
            return_old,
        } = update;
        let alignment = if order.is_some() {
            Self::atomic_pointer_alignment(place)
        } else {
            Self::atomic_lvalue_alignment(place)
        };
        let size = object.layout().unwrap().size;
        let fp = [object, lhs_type, rhs_type]
            .iter()
            .any(|ty| matches!(ty.kind, Kind::Float(_) | Kind::LongDouble));
        let frame = if fp { 96 } else { 64 };
        if order.is_some() {
            self.value(place)?;
        } else {
            self.address(place)?;
        }
        self.adjust_stack("sub", frame);
        self.text.push_str("    mov qword [rsp], rax\n");
        if let Some(rhs) = rhs {
            self.converted(rhs, rhs_type)?;
        } else {
            match rhs_type.kind {
                Kind::Float(Float::Single) => self.float_constant(0x3f80_0000, Float::Single, 0),
                Kind::Float(Float::Double) => {
                    self.float_constant(0x3ff0_0000_0000_0000, Float::Double, 0)
                }
                Kind::LongDouble => self.text.push_str("    fld1\n"),
                _ => self.text.push_str("    mov eax, 1\n"),
            }
        }
        self.store(rhs_type, "[rsp + 16]");
        if let Some(order) = order {
            self.converted(&order.expr, &order.ty)?;
        } else {
            writeln!(self.text, "    mov eax, {SEQ_CST}").unwrap();
        }
        self.text.push_str("    mov dword [rsp + 8], eax\n");
        if fp {
            self.use_symbol("feholdexcept", place.position())?;
            self.use_symbol("feclearexcept", place.position())?;
            self.use_symbol("feupdateenv", place.position())?;
            self.text
                .push_str("    lea rdi, [rsp + 64]\n    call feholdexcept@plt\n");
        }
        let expected = "[rsp + 32]";
        let desired = "[rsp + 48]";
        self.atomic_access(size, alignment, |emitter, width| {
            if let Some(Width { memory, accumulator, .. }) = width {
                writeln!(
                    emitter.text,
                    "    mov {accumulator}, {memory} ptr [r11]\n    mov {memory} ptr {expected}, {accumulator}"
                )
                .unwrap();
            } else {
                // A relaxed initial read suffices. The successful RMW
                // carries the requested order and failures retry relaxed.
                writeln!(emitter.text, "    lea rdx, {expected}\n    xor ecx, ecx").unwrap();
                emitter.brt_call("__atomic_load", size);
            }
        });
        let retry = self.label();
        let success = self.label();
        writeln!(self.text, "{retry}:").unwrap();
        self.load(object, expected);
        self.convert(object, lhs_type);
        self.adjust_stack("sub", 16);
        self.store(lhs_type, "[rsp]");
        self.load(rhs_type, "[rsp + 32]");
        self.combine(op, lhs_type, rhs_type, lhs_type);
        self.convert(lhs_type, object);
        self.write_atomic_buffer(object, desired);

        self.atomic_access(size, alignment, |emitter, width| {
            if let Some(Width { memory, accumulator, data }) = width {
                writeln!(
                    emitter.text,
                    "    mov {data}, {memory} ptr {desired}\n    mov {accumulator}, {memory} ptr {expected}\n    lock cmpxchg {memory} ptr [r11], {data}\n    je {success}\n    mov {memory} ptr {expected}, {accumulator}"
                )
                .unwrap();
            } else {
                writeln!(
                    emitter.text,
                    "    lea rdx, {expected}\n    lea rcx, {desired}\n    mov r8d, dword [rsp + 8]\n    xor r9d, r9d"
                )
                .unwrap();
                emitter.brt_call("__atomic_compare_exchange", size);
                writeln!(emitter.text, "    test al, al\n    jne {success}").unwrap();
            }
        });
        if fp {
            // As on the inline path, discard exceptions from failed attempts.
            self.text
                .push_str("    mov edi, 0x3d\n    call feclearexcept@plt\n");
        }
        writeln!(self.text, "    jmp {retry}\n{success}:").unwrap();
        if fp {
            self.text
                .push_str("    lea rdi, [rsp + 64]\n    call feupdateenv@plt\n");
        }
        self.load(object, if return_old { expected } else { desired });
        self.adjust_stack("add", frame);
        Ok(())
    }
}
