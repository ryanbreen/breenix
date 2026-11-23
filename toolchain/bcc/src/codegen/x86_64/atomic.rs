//! N1570 6.5.2.4, 6.5.16.2 and 7.17. Intel SDM Vol. 3A 9.1–9.2
//! (December 2023): aligned scalar loads and stores are indivisible.
//! Locked instructions have a total order and prevent StoreLoad reordering.

use super::*;
use crate::sema::SEQ_CST;

struct Width {
    memory: &'static str,
    accumulator: &'static str,
    data: &'static str,
}

impl Width {
    fn of(ty: &Type) -> Self {
        let (memory, accumulator, data) = match ty.layout().unwrap().size {
            1 => ("byte", "al", "dl"),
            2 => ("word", "ax", "dx"),
            4 => ("dword", "eax", "edx"),
            8 => ("qword", "rax", "rdx"),
            _ => unreachable!("sema checks atomic widths"),
        };
        Self {
            memory,
            accumulator,
            data,
        }
    }
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn atomic_builtin(
        &mut self,
        op: AtomicOp,
        args: &[Argument<'src>],
    ) -> Result<(), Diagnostic> {
        use AtomicOp::*;
        match op {
            ThreadFence | SignalFence => return self.atomic_fence(op, &args[0]),
            IsLockFree => {
                self.converted(&args[0].expr, &args[0].ty)?;
                // 1, 2, 4 and 8 are the nonzero powers of two up to eight.
                self.text.push_str(
                    "    lea rdx, [rax - 1]\n    cmp rdx, 7\n    setbe cl\n    test rax, rdx\n    sete al\n    and al, cl\n    movzx eax, al\n",
                );
                return Ok(());
            }
            _ => {}
        }
        let Kind::Pointer(object) = &args[0].ty.kind else {
            unreachable!("sema checks atomic pointer arguments");
        };
        let width = Width::of(object);
        let Width {
            memory,
            accumulator,
            data,
        } = width;

        self.value(&args[0].expr)?;
        self.adjust_stack("sub", 32);
        self.text.push_str("    mov qword [rsp], rax\n");
        let order_index = match op {
            Load => 1,
            CompareExchange => 3,
            _ => 2,
        };
        for (index, arg) in args.iter().enumerate().take(order_index).skip(1) {
            self.converted(&arg.expr, &arg.ty)?;
            self.atomic_value_to_bits(&arg.ty);
            writeln!(self.text, "    mov qword [rsp + {}], rax", index * 8).unwrap();
        }
        // The weaker orders share the same instructions on AMD64. Dynamic
        // arguments still run once, including the compare-exchange failure order.
        let mut seq_cst = Some(false);
        for order in &args[order_index..] {
            seq_cst = self.atomic_order(order)?;
        }
        self.text.push_str("    mov r11, qword [rsp]\n");
        match op {
            Init | Store => {
                self.text.push_str("    mov rdx, qword [rsp + 8]\n");
                self.atomic_store(width, seq_cst);
            }
            Load => self.load(object, "[r11]"),
            Exchange => {
                writeln!(
                    self.text,
                    "    mov rax, qword [rsp + 8]\n    xchg {memory} ptr [r11], {accumulator}",
                )
                .unwrap();
                self.atomic_bits_to_value(object);
            }
            CompareExchange => self.atomic_compare_exchange(width),
            Fetch(op) => {
                if matches!(op, Binary::Add | Binary::Subtract) {
                    self.text.push_str("    mov rdx, qword [rsp + 8]\n");
                    if let Kind::Pointer(element) = &object.kind {
                        self.scale_pointer_index("rdx", element);
                    }
                    if op == Binary::Subtract {
                        self.text.push_str("    neg rdx\n");
                    }
                    writeln!(
                        self.text,
                        "    lock xadd {memory} ptr [r11], {data}\n    mov rax, rdx"
                    )
                    .unwrap();
                } else {
                    let retry = self.label();
                    let instruction = match op {
                        Binary::BitAnd => "and",
                        Binary::BitOr => "or",
                        Binary::BitXor => "xor",
                        _ => unreachable!(),
                    };
                    writeln!(
                        self.text,
                        "    mov {accumulator}, {memory} ptr [r11]\n{retry}:\n    mov rdx, rax\n    {instruction} rdx, qword [rsp + 8]\n    lock cmpxchg {memory} ptr [r11], {data}\n    jne {retry}",
                    )
                    .unwrap();
                }
                self.atomic_bits_to_value(object);
            }
            ThreadFence | SignalFence | IsLockFree => unreachable!(),
        }
        self.adjust_stack("add", 32);
        Ok(())
    }

    fn atomic_order(&mut self, order: &Argument<'src>) -> Result<Option<bool>, Diagnostic> {
        if let ExpressionKind::Integer(value) = order.expr.kind {
            Ok(Some(value as u32 == SEQ_CST))
        } else {
            self.converted(&order.expr, &order.ty)?;
            Ok(None)
        }
    }

    fn atomic_fence(&mut self, op: AtomicOp, order: &Argument<'src>) -> Result<(), Diagnostic> {
        let seq_cst = self.atomic_order(order)?;
        // bcc emits accesses in source order at -O0. A signal fence and
        // acquire/release thread fences require no additional instruction.
        if op == AtomicOp::ThreadFence {
            match seq_cst {
                Some(true) => self.text.push_str("    mfence\n"),
                Some(false) => {}
                None => {
                    let done = self.label();
                    writeln!(
                        self.text,
                        "    cmp eax, {SEQ_CST}\n    jne {done}\n    mfence\n{done}:"
                    )
                    .unwrap();
                }
            }
        }
        Ok(())
    }

    fn atomic_store(&mut self, width: Width, seq_cst: Option<bool>) {
        let Width { memory, data, .. } = width;
        // A release store uses MOV. The locked XCHG for a seq_cst store
        // also orders subsequent loads, which would otherwise pass it.
        match seq_cst {
            Some(true) => writeln!(self.text, "    xchg {memory} ptr [r11], {data}").unwrap(),
            Some(false) => writeln!(self.text, "    mov {memory} ptr [r11], {data}").unwrap(),
            None => {
                let relaxed = self.label();
                let done = self.label();
                writeln!(
                    self.text,
                    "    cmp eax, {SEQ_CST}\n    jne {relaxed}\n    xchg {memory} ptr [r11], {data}\n    jmp {done}\n{relaxed}:\n    mov {memory} ptr [r11], {data}\n{done}:",
                )
                .unwrap();
            }
        }
    }

    fn atomic_compare_exchange(&mut self, width: Width) {
        let Width {
            memory,
            accumulator,
            data,
        } = width;
        let success = self.label();
        let done = self.label();
        // AMD64 CMPXCHG can implement both C11 forms. On failure it
        // supplies the observed bits, which replace *expected.
        writeln!(
            self.text,
            "    mov r10, qword [rsp + 8]\n    mov rdx, qword [rsp + 16]\n    mov {accumulator}, {memory} ptr [r10]\n    lock cmpxchg {memory} ptr [r11], {data}\n    je {success}\n    mov {memory} ptr [r10], {accumulator}\n    xor eax, eax\n    jmp {done}\n{success}:\n    mov eax, 1\n{done}:",
        )
        .unwrap();
    }

    pub(super) fn atomic_assign(&mut self, ty: &Type) {
        // Preserve the converted assignment result in its canonical register.
        match ty.kind {
            Kind::Float(Float::Single) => self.text.push_str("    movd edx, xmm0\n"),
            Kind::Float(Float::Double) => self.text.push_str("    movq rdx, xmm0\n"),
            _ => self.text.push_str("    mov rdx, rax\n"),
        }
        self.atomic_store(Width::of(ty), Some(true));
    }

    pub(super) fn atomic_rmw(
        &mut self,
        left: &Expression<'src>,
        right: Option<&Expression<'src>>,
        op: Binary,
        (left_ty, right_ty): (&Type, &Type),
        postfix: bool,
    ) -> Result<(), Diagnostic> {
        let object = &left.ty;
        let Width {
            memory,
            accumulator,
            data,
        } = Width::of(object);
        let fp = [object, left_ty, right_ty]
            .iter()
            .any(|ty| matches!(ty.kind, Kind::Float(_) | Kind::LongDouble));
        // Address, converted RHS, observed bits, and (for floating
        // operations) the AMD64 fenv_t from our libc's fenv.h.
        let frame = if fp { 80 } else { 48 };
        self.address(left)?;
        self.adjust_stack("sub", frame);
        self.text.push_str("    mov qword [rsp], rax\n");
        if let Some(right) = right {
            self.converted(right, right_ty)?;
        } else if let Kind::Float(float) = right_ty.kind {
            let bits = match float {
                Float::Single => 0x3f80_0000,
                Float::Double => 0x3ff0_0000_0000_0000,
            };
            self.float_constant(bits, float, 0);
        } else {
            self.text.push_str("    mov eax, 1\n");
        }
        self.store(right_ty, "[rsp + 16]");
        if fp {
            // N1570 6.5.16.2p3 note 113: an unsuccessful attempt must
            // discard its exceptions without changing the live rounding mode.
            self.use_symbol("feholdexcept", left.position())?;
            self.use_symbol("feclearexcept", left.position())?;
            self.use_symbol("feupdateenv", left.position())?;
            self.text
                .push_str("    lea rdi, [rsp + 48]\n    call feholdexcept@plt\n");
        }
        let retry = self.label();
        writeln!(
            self.text,
            "    mov r11, qword [rsp]\n    mov {accumulator}, {memory} ptr [r11]\n{retry}:\n    mov qword [rsp + 32], rax",
        )
        .unwrap();
        self.load(object, "[rsp + 32]");
        self.convert(object, left_ty);
        self.adjust_stack("sub", 16);
        self.store(left_ty, "[rsp]");
        self.load(right_ty, "[rsp + 32]");
        self.combine(op, left_ty, right_ty, left_ty);
        self.convert(left_ty, object);
        self.atomic_value_to_bits(object);
        writeln!(
            self.text,
            "    mov rdx, rax\n    mov rax, qword [rsp + 32]\n    mov r11, qword [rsp]\n    lock cmpxchg {memory} ptr [r11], {data}",
        )
        .unwrap();
        if fp {
            let success = self.label();
            // FE_ALL_EXCEPT is 0x3d on x86-64 (Breenix fenv.h).
            writeln!(
                self.text,
                "    je {success}\n    mov qword [rsp + 32], rax\n    mov edi, 0x3d\n    call feclearexcept@plt\n    mov rax, qword [rsp + 32]\n    jmp {retry}\n{success}:",
            )
            .unwrap();
        } else {
            writeln!(self.text, "    jne {retry}").unwrap();
        }
        if !postfix {
            self.text.push_str("    mov rax, rdx\n");
        }
        if fp {
            self.text.push_str(
                "    mov qword [rsp + 32], rax\n    lea rdi, [rsp + 48]\n    call feupdateenv@plt\n    mov rax, qword [rsp + 32]\n",
            );
        }
        self.atomic_bits_to_value(object);
        self.adjust_stack("add", frame);
        Ok(())
    }

    fn atomic_value_to_bits(&mut self, ty: &Type) {
        match ty.kind {
            Kind::Float(Float::Single) => self.text.push_str("    movd eax, xmm0\n"),
            Kind::Float(Float::Double) => self.text.push_str("    movq rax, xmm0\n"),
            _ => {}
        }
    }

    fn atomic_bits_to_value(&mut self, ty: &Type) {
        match ty.kind {
            Kind::Float(Float::Single) => self.text.push_str("    movd xmm0, eax\n"),
            Kind::Float(Float::Double) => self.text.push_str("    movq xmm0, rax\n"),
            _ => self.normalize_integer(ty),
        }
    }
}
