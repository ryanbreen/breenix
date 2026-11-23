//! N1570 §§6.5.2.4, 6.5.16.2 and 7.17. Orders follow GCC 15.2 §7.9.1.
//! Arm DDI 0487 M.b §§C6.2.89, .150, .191–198, .265–267, .401–411
//! and .449–451 define the barriers and aligned single-copy/exclusive accesses.

use super::*;
use crate::sema::{constant_integer, AtomicOp};
use crate::types::ARM_ATOMIC_RUNTIME;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Relaxed,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
}

impl Order {
    fn acquire(self) -> bool {
        matches!(self, Self::Acquire | Self::AcqRel | Self::SeqCst)
    }

    fn release(self) -> bool {
        matches!(self, Self::Release | Self::AcqRel | Self::SeqCst)
    }
}

#[derive(Clone, Copy)]
struct Width {
    suffix: &'static str,
    register: char,
}

impl Width {
    fn of(size: usize) -> Option<Self> {
        let (suffix, register) = match size {
            1 => ("b", 'w'),
            2 => ("h", 'w'),
            4 => ("", 'w'),
            8 => ("", 'x'),
            _ => return None,
        };
        Some(Self { suffix, register })
    }

    fn transfer(self, out: &mut String, op: &str, reg: usize, address: &str) {
        let Self { suffix, register } = self;
        writeln!(out, "    {op}{suffix} {register}{reg}, [{address}]").unwrap();
    }

    fn store_exclusive(self, out: &mut String, order: Order) {
        let op = if order.release() { "stlxr" } else { "stxr" };
        let Self { suffix, register } = self;
        writeln!(out, "    {op}{suffix} w14, {register}12, [x11]").unwrap();
    }
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn atomic_plain_load(&mut self, place: &Expression<'src>) -> Result<(), Diagnostic> {
        let width = atomic_width(&place.ty, lvalue_alignment(place), place.position())?;
        self.address(place)?;
        width.transfer(&mut self.text, "ldar", 0, "x0");
        self.atomic_bits_to_value(&place.ty, place.position())
    }

    pub(super) fn atomic_assignment(
        &mut self,
        place: &Expression<'src>,
        rhs: &Expression<'src>,
        operation: Option<&(Binary, Type, Type)>,
    ) -> Result<(), Diagnostic> {
        let object = &place.ty;
        if let Some((op, left, right)) = operation {
            return self.atomic_transform(place, Some((rhs, right)), false, |emitter| {
                emitter.convert(object, left);
                emitter.push_value(value_prefix(left));
                let register = value_prefix(right);
                writeln!(emitter.text, "    ldr {register}0, [sp, #32]").unwrap();
                emitter.combine(*op, left, right, left);
                emitter.convert(left, object);
            });
        }
        let width = atomic_width(object, lvalue_alignment(place), place.position())?;
        self.address(place)?;
        self.push_value('x');
        self.converted(rhs, object)?;
        self.atomic_value_to_bits(object);
        self.text.push_str("    ldr x11, [sp], #16\n");
        self.atomic_store(width, 0, Order::SeqCst);
        self.atomic_bits_to_value(object, place.position())
    }

    pub(super) fn atomic_record_initialize(&mut self, ty: &Type, local: usize, offset: usize) {
        self.atomic_value_to_bits(ty);
        self.local_address(local, offset, "x10");
        // Initialization is non-atomic (N1570 7.17.2.2p2). Even a record
        // within a packed initializer can be written byte by byte.
        let layout = Layout {
            align: 1,
            ..ty.layout().unwrap()
        };
        self.write_atomic_bits(layout, "x10");
    }

    pub(super) fn atomic_update(
        &mut self,
        place: &Expression<'src>,
        increment: bool,
        postfix: bool,
    ) -> Result<(), Diagnostic> {
        self.atomic_transform(place, None, postfix, |emitter| {
            let object = &place.ty;
            if let Kind::Float(precision) = object.kind {
                let fp = float::prefix(precision);
                let op = if increment { "fadd" } else { "fsub" };
                writeln!(
                    emitter.text,
                    "    fmov {fp}1, #1.0\n    {op} {fp}0, {fp}0, {fp}1"
                )
                .unwrap();
            } else {
                let step = match &object.kind {
                    Kind::Pointer(element) => element.layout().unwrap().size,
                    _ => 1,
                };
                let op = if increment { "add" } else { "sub" };
                emitter.add_sub_offset(op, "x0", "x0", step as u64);
                if object.kind == Kind::Integer(Integer::Bool) {
                    emitter.boolean(object);
                }
            }
        })
    }

    fn atomic_transform(
        &mut self,
        place: &Expression<'src>,
        rhs: Option<(&Expression<'src>, &Type)>,
        postfix: bool,
        compute: impl FnOnce(&mut Self),
    ) -> Result<(), Diagnostic> {
        let object = &place.ty;
        let width = atomic_width(object, lvalue_alignment(place), place.position())?;
        self.address(place)?;
        self.push_value('x');
        let address_offset = if let Some((value, ty)) = rhs {
            self.converted(value, ty)?;
            self.push_value(value_prefix(ty));
            16
        } else {
            0
        };

        // Compute from a snapshot before taking the exclusive reservation.
        // The promoted operation may spill, or even call a binary128 helper.
        // The exclusive pair commits only if the snapshot still matches.
        let retry = self.label();
        let changed = self.label();
        let done = self.label();
        writeln!(self.text, "{retry}:\n    ldr x11, [sp, #{address_offset}]").unwrap();
        width.transfer(&mut self.text, "ldr", 0, "x11");
        self.push_value('x');
        self.atomic_bits_to_value(object, place.position())?;
        compute(self);
        self.atomic_value_to_bits(object);
        self.text
            .push_str("    mov x12, x0\n    ldr x13, [sp], #16\n");
        writeln!(self.text, "    ldr x11, [sp, #{address_offset}]").unwrap();
        width.transfer(&mut self.text, "ldaxr", 0, "x11");
        writeln!(self.text, "    cmp x0, x13\n    b.ne {changed}").unwrap();
        width.store_exclusive(&mut self.text, Order::SeqCst);
        writeln!(self.text, "    cbnz w14, {retry}\n    b {done}\n{changed}:").unwrap();
        self.clear_exclusive();
        writeln!(self.text, "    b {retry}\n{done}:").unwrap();
        self.adjust_stack("add", address_offset + 16);
        if !postfix {
            // The store truncated to width, so drop any high bits before decoding.
            self.text.push_str("    mov x0, x12\n");
        }
        self.atomic_bits_to_value(object, place.position())
    }

    pub(super) fn atomic_builtin(
        &mut self,
        op: AtomicOp,
        weak: bool,
        args: &[Argument<'src>],
    ) -> Result<(), Diagnostic> {
        use AtomicOp::*;

        match op {
            ThreadFence | SignalFence => {
                let order = self.atomic_order(&args[0])?;
                // Instruction order is preserved at -O0. A signal fence
                // needs no machine instruction.
                if op == ThreadFence {
                    self.text.push_str(match order {
                        Order::Relaxed => "",
                        Order::Acquire => "    dmb ishld\n",
                        _ => "    dmb ish\n",
                    });
                }
                return Ok(());
            }
            IsLockFree => {
                self.converted(&args[0].expr, &args[0].ty)?;
                // Nonzero powers of two up to eight.
                self.text.push_str(
                    "    sub x9, x0, #1\n    cmp x9, #7\n    cset w10, ls\n    tst x0, x9\n    cset w0, eq\n    and w0, w0, w10\n",
                );
                return Ok(());
            }
            _ => {}
        }

        let pointer = &args[0].expr;
        let Kind::Pointer(object) = &args[0].ty.kind else {
            unreachable!("sema checks atomic pointer arguments");
        };
        if op == IsLockFreeAt {
            self.value(pointer)?;
            let layout = object.layout().unwrap();
            match layout.size {
                1 => self.text.push_str("    mov w0, #1\n"),
                2 | 4 | 8 if layout.is_lock_free_atomic() => {
                    writeln!(
                        self.text,
                        "    tst x0, #{}\n    cset w0, eq",
                        layout.size - 1
                    )
                    .unwrap();
                }
                _ => self.text.push_str("    mov w0, #0\n"),
            }
            return Ok(());
        }
        let width = atomic_width(object, pointer_alignment(pointer), pointer.position())?;

        // Evaluate and spill each argument before entering the exclusive loop.
        self.value(pointer)?;
        self.push_value('x');
        let mut index = 1;
        if op == CompareExchange {
            self.value(&args[index].expr)?;
            self.push_value('x');
            index += 1;
        }
        if op != Load {
            let value = &args[index];
            self.converted(&value.expr, &value.ty)?;
            self.atomic_value_to_bits(&value.ty);
            if matches!(op, Fetch(Binary::Add | Binary::Subtract)) {
                if let Kind::Pointer(element) = &object.kind {
                    self.scale_index("x0", element);
                }
            }
            self.push_value('x');
            index += 1;
        }
        let order = if op == Init {
            Order::Relaxed
        } else {
            self.atomic_order(&args[index])?
        };
        let failure = if op == CompareExchange {
            self.atomic_order(&args[index + 1])?
        } else {
            Order::Relaxed
        };
        if op != Load {
            self.text.push_str("    ldr x12, [sp], #16\n");
        }
        if op == CompareExchange {
            self.text.push_str("    ldr x10, [sp], #16\n");
        }
        self.text.push_str("    ldr x11, [sp], #16\n");

        match op {
            Init | Store => self.atomic_store(width, 12, order),
            Load => {
                let instruction = if order.acquire() { "ldar" } else { "ldr" };
                width.transfer(&mut self.text, instruction, 0, "x11");
                self.atomic_bits_to_value(object, pointer.position())?;
            }
            Exchange | Fetch(_) => {
                self.atomic_rmw(width, op, order);
                self.atomic_bits_to_value(object, pointer.position())?;
            }
            CompareExchange => {
                self.atomic_compare_exchange(width, object, order, failure, weak);
            }
            ThreadFence | SignalFence | IsLockFree | IsLockFreeAt => unreachable!(),
        }
        Ok(())
    }

    fn atomic_order(&mut self, argument: &Argument<'src>) -> Result<Order, Diagnostic> {
        let order = match constant_integer(&argument.expr, Target::Aarch64) {
            Some(0) => Order::Relaxed,
            Some(1 | 2) => Order::Acquire, // Consume is implemented as acquire.
            Some(3) => Order::Release,
            Some(4) => Order::AcqRel,
            Some(_) => Order::SeqCst,
            None => {
                self.converted(&argument.expr, &argument.ty)?;
                // GCC §7.9.1 allows run-time orders to use the strongest order.
                Order::SeqCst
            }
        };
        Ok(order)
    }

    fn atomic_store(&mut self, width: Width, reg: usize, order: Order) {
        let op = if order.release() { "stlr" } else { "str" };
        width.transfer(&mut self.text, op, reg, "x11");
        if order == Order::SeqCst {
            // Keep subsequent accesses ordered after the SC store.
            self.text.push_str("    dmb ish\n");
        }
    }

    fn atomic_rmw(&mut self, width: Width, op: AtomicOp, order: Order) {
        let load = if order.acquire() { "ldaxr" } else { "ldxr" };
        // x13 holds the operand across retries. x0 holds the old value;
        // w14 is distinct from both the base and the store operand.
        if matches!(op, AtomicOp::Fetch(_)) {
            self.text.push_str("    mov x13, x12\n");
        }
        let retry = self.label();
        writeln!(self.text, "{retry}:").unwrap();
        width.transfer(&mut self.text, load, 0, "x11");
        if let AtomicOp::Fetch(op) = op {
            let instruction = match op {
                Binary::Add => "add",
                Binary::Subtract => "sub",
                Binary::BitAnd => "and",
                Binary::BitOr => "orr",
                Binary::BitXor => "eor",
                _ => unreachable!("sema checks fetch operators"),
            };
            writeln!(self.text, "    {instruction} x12, x0, x13").unwrap();
        }
        width.store_exclusive(&mut self.text, order);
        writeln!(self.text, "    cbnz w14, {retry}").unwrap();
    }

    fn atomic_compare_exchange(
        &mut self,
        width: Width,
        object: &Type,
        success: Order,
        failure: Order,
        weak: bool,
    ) {
        let expected = object.unqualified().layout().unwrap();
        self.read_atomic_bits(expected, 13, "x10");
        let retry = self.label();
        let mismatch = self.label();
        let failed = self.label();
        let done = self.label();
        writeln!(self.text, "{retry}:").unwrap();
        let load = if success.acquire() || failure.acquire() {
            "ldaxr"
        } else {
            "ldxr"
        };
        width.transfer(&mut self.text, load, 0, "x11");
        writeln!(self.text, "    cmp x0, x13\n    b.ne {mismatch}").unwrap();
        width.store_exclusive(&mut self.text, success);
        let on_lost_reservation = if weak { &failed } else { &retry };
        writeln!(
            self.text,
            "    cbnz w14, {on_lost_reservation}\n    mov w0, #1\n    b {done}\n{mismatch}:"
        )
        .unwrap();
        self.clear_exclusive();
        writeln!(self.text, "{failed}:").unwrap();
        self.write_atomic_bits(expected, "x10");
        writeln!(self.text, "    mov w0, #0\n{done}:").unwrap();
    }

    fn clear_exclusive(&mut self) {
        // DDI 0487 M.b §C6.2.89, CLREX with the default immediate (15).
        // bas does not have a mnemonic for this instruction.
        self.text.push_str("    .long 0xd5033f5f\n");
    }

    fn atomic_value_to_bits(&mut self, ty: &Type) {
        match ty.kind {
            Kind::Float(Float::Single) => self.text.push_str("    fmov w0, s0\n"),
            Kind::Float(Float::Double) => self.text.push_str("    fmov x0, d0\n"),
            Kind::Record(_) => {
                self.text.push_str("    mov x10, x0\n");
                self.read_atomic_bits(ty.unqualified().layout().unwrap(), 0, "x10");
            }
            _ => {}
        }
    }

    fn atomic_bits_to_value(&mut self, ty: &Type, site: &Position) -> Result<(), Diagnostic> {
        match ty.kind {
            Kind::Float(Float::Single) => self.text.push_str("    fmov s0, w0\n"),
            Kind::Float(Float::Double) => self.text.push_str("    fmov d0, x0\n"),
            Kind::Record(_) => {
                // The aggregate value is a snapshot, independent of other threads.
                let layout = ty.unqualified().layout().unwrap();
                let distance = self.frame_slot(layout, site)?;
                self.offset_address("x10", "x29", -(distance as i64));
                self.write_atomic_bits(layout, "x10");
                self.text.push_str("    mov x0, x10\n");
            }
            _ => self.normalize(ty),
        }
        Ok(())
    }

    // An ordinary record can be smaller or less aligned than its atomic
    // representation. Widen the representation without reading past it.
    fn read_atomic_bits(&mut self, layout: Layout, reg: usize, address: &str) {
        if layout.is_lock_free_atomic() {
            Width::of(layout.size)
                .unwrap()
                .transfer(&mut self.text, "ldr", reg, address);
        } else {
            writeln!(self.text, "    mov x{reg}, xzr").unwrap();
            for byte in 0..layout.size {
                writeln!(
                    self.text,
                    "    ldrb w9, [{address}, #{byte}]\n    orr x{reg}, x{reg}, x9, lsl #{}",
                    byte * 8
                )
                .unwrap();
            }
        }
    }

    fn write_atomic_bits(&mut self, layout: Layout, address: &str) {
        if layout.is_lock_free_atomic() {
            Width::of(layout.size)
                .unwrap()
                .transfer(&mut self.text, "str", 0, address);
        } else {
            for byte in 0..layout.size {
                writeln!(
                    self.text,
                    "    lsr x9, x0, #{}\n    strb w9, [{address}, #{byte}]",
                    byte * 8
                )
                .unwrap();
            }
        }
    }
}

fn atomic_width(ty: &Type, alignment: usize, site: &Position) -> Result<Width, Diagnostic> {
    ty.layout()
        .filter(|layout| layout.is_lock_free_atomic() && alignment >= layout.size)
        .and_then(|layout| Width::of(layout.size))
        .ok_or_else(|| Diagnostic::new(site, ARM_ATOMIC_RUNTIME))
}

fn pointer_alignment(pointer: &Expression<'_>) -> usize {
    match &pointer.kind {
        ExpressionKind::Unary(Unary::Address, place) => lvalue_alignment(place),
        _ => match &pointer.ty.kind {
            Kind::Pointer(pointee) => pointee.layout().map_or(1, |layout| layout.align),
            Kind::Array(element, _) => element.layout().map_or(1, |layout| layout.align),
            _ => 1,
        },
    }
}

fn lvalue_alignment(place: &Expression<'_>) -> usize {
    match &place.kind {
        ExpressionKind::Unary(Unary::Dereference, pointer) => pointer_alignment(pointer),
        ExpressionKind::Member { base, offset, .. } => {
            let base_alignment = lvalue_alignment(base);
            if offset.is_multiple_of(base_alignment) {
                base_alignment
            } else {
                1 << offset.trailing_zeros()
            }
        }
        _ => place.ty.layout().map_or(1, |layout| layout.align),
    }
}
