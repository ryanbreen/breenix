//! N1570 §§6.5.2.4, 6.5.16.2 and 7.17. Orders follow GCC 15.2 §7.9.1.
//! Arm DDI 0487 M.b §§C6.2.89, .150, .191–198, .265–267, .401–411
//! and .449–451 define the barriers and aligned single-copy/exclusive accesses.
//! Generic libcalls follow LLVM 22.1.0 and AAPCS64 §6.8.2.

use super::*;
use crate::sema::{constant_integer, AtomicOp};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Relaxed = 0,
    Acquire = 2,
    Release = 3,
    AcqRel = 4,
    SeqCst = 5,
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
        self.address(place)?;
        self.text.push_str("    mov x11, x0\n");
        self.atomic_load_at(
            &place.ty,
            lvalue_alignment(place),
            Order::SeqCst,
            place.position(),
        )
    }

    pub(super) fn atomic_assignment(
        &mut self,
        place: &Expression<'src>,
        rhs: &Expression<'src>,
        operation: Option<&(Binary, Type, Type)>,
    ) -> Result<(), Diagnostic> {
        let object = &place.ty;
        let site = place.position();
        let alignment = lvalue_alignment(place);
        self.address(place)?;
        self.push_value('x');
        if let Some((op, left, right)) = operation {
            let slot = self.frame_slot(right.layout().unwrap(), site)?;
            self.converted(rhs, right)?;
            self.atomic_slot_address(slot, "x9");
            self.store_value(right, None);
            return self.atomic_transform(object, alignment, false, site, |emitter| {
                emitter.convert(object, left);
                emitter.push_value(value_prefix(left));
                emitter.read_atomic_slot(right, slot);
                emitter.combine(*op, left, right, left);
                emitter.convert(left, object);
            });
        }
        let slot = self.stage_atomic_value(rhs, object, site)?;
        self.text.push_str("    ldr x11, [sp], #16\n");
        self.atomic_write_at(
            AtomicOp::Store,
            object,
            alignment,
            slot,
            Order::SeqCst,
            site,
        )?;
        self.read_atomic_slot(object, slot);
        Ok(())
    }

    pub(super) fn atomic_record_initialize(&mut self, ty: &Type, local: usize, offset: usize) {
        self.local_address(local, offset, "x9");
        self.write_atomic_value(ty);
    }

    pub(super) fn atomic_update(
        &mut self,
        place: &Expression<'src>,
        increment: bool,
        postfix: bool,
    ) -> Result<(), Diagnostic> {
        let object = &place.ty;
        self.address(place)?;
        self.push_value('x');
        self.atomic_transform(
            object,
            lvalue_alignment(place),
            postfix,
            place.position(),
            |emitter| {
                if let Kind::Float(precision) = object.kind {
                    let fp = float::prefix(precision);
                    let op = if increment { "fadd" } else { "fsub" };
                    writeln!(
                        emitter.text,
                        "    fmov {fp}1, #1.0\n    {op} {fp}0, {fp}0, {fp}1"
                    )
                    .unwrap();
                } else if object.kind == Kind::LongDouble {
                    emitter.binary128_step(increment);
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
            },
        )
    }

    fn atomic_transform(
        &mut self,
        object: &Type,
        alignment: usize,
        postfix: bool,
        site: &Position,
        compute: impl Fn(&mut Self),
    ) -> Result<(), Diagnostic> {
        self.text.push_str("    ldr x11, [sp]\n");
        self.atomic_access(
            object,
            alignment,
            |emitter, width| {
                emitter.atomic_inline_transform(object, width, postfix, site, &compute)
            },
            |emitter| {
                emitter.atomic_runtime_transform(object, Order::SeqCst, postfix, site, &compute)
            },
        )?;
        self.adjust_stack("add", 16);
        Ok(())
    }

    fn atomic_inline_transform(
        &mut self,
        object: &Type,
        width: Width,
        postfix: bool,
        site: &Position,
        compute: impl FnOnce(&mut Self),
    ) -> Result<(), Diagnostic> {
        // Compute from a snapshot before taking the exclusive reservation.
        // The promoted operation may spill, or even call a binary128 helper.
        // The exclusive pair commits only if the snapshot still matches.
        let retry = self.label();
        let changed = self.label();
        let done = self.label();
        writeln!(self.text, "{retry}:\n    ldr x11, [sp]").unwrap();
        width.transfer(&mut self.text, "ldr", 0, "x11");
        self.push_value('x');
        self.atomic_bits_to_value(object, site)?;
        compute(self);
        self.atomic_value_to_bits(object);
        self.text
            .push_str("    mov x12, x0\n    ldr x13, [sp], #16\n    ldr x11, [sp]\n");
        width.transfer(&mut self.text, "ldaxr", 0, "x11");
        writeln!(self.text, "    cmp x0, x13\n    b.ne {changed}").unwrap();
        width.store_exclusive(&mut self.text, Order::SeqCst);
        writeln!(self.text, "    cbnz w14, {retry}\n    b {done}\n{changed}:").unwrap();
        self.clear_exclusive();
        writeln!(self.text, "    b {retry}\n{done}:").unwrap();
        if !postfix {
            // The store truncated to width, so drop any high bits before decoding.
            self.text.push_str("    mov x0, x12\n");
        }
        self.atomic_bits_to_value(object, site)
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
        let site = pointer.position();
        let alignment = pointer_alignment(pointer);
        // Preserve the pointer across nested expressions and calls.
        self.value(pointer)?;
        self.push_value('x');
        match op {
            Load => {
                let order = self.atomic_order(&args[1])?;
                self.text.push_str("    ldr x11, [sp]\n");
                self.atomic_load_at(object, alignment, order, site)?;
            }
            Init | Store | Exchange => {
                let slot = self.stage_atomic_value(&args[1].expr, object, site)?;
                let order = if op == Init {
                    Order::Relaxed
                } else {
                    self.atomic_order(&args[2])?
                };
                self.text.push_str("    ldr x11, [sp]\n");
                self.atomic_write_at(op, object, alignment, slot, order, site)?;
            }
            CompareExchange => {
                self.value(&args[1].expr)?;
                self.push_value('x');
                let desired = self.stage_atomic_value(&args[2].expr, object, site)?;
                let success = self.atomic_order(&args[3])?;
                let failure = self.atomic_order(&args[4])?;
                self.text.push_str("    ldr x11, [sp, #16]\n");
                self.atomic_access(
                    object,
                    alignment,
                    |emitter, width| {
                        emitter.atomic_slot_address(desired, "x9");
                        width.transfer(&mut emitter.text, "ldr", 12, "x9");
                        emitter.text.push_str("    ldr x10, [sp]\n");
                        emitter.atomic_compare_exchange(width, object, success, failure, weak);
                        Ok(())
                    },
                    |emitter| emitter.atomic_runtime_cas(object, desired, success, failure, site),
                )?;
                self.adjust_stack("add", 16);
            }
            Fetch(binary) => {
                let rhs = &args[1];
                let slot = self.stage_atomic_value(&rhs.expr, &rhs.ty, site)?;
                let order = self.atomic_order(&args[2])?;
                self.text.push_str("    ldr x11, [sp]\n");
                self.atomic_access(
                    object,
                    alignment,
                    |emitter, width| {
                        emitter.read_atomic_slot(&rhs.ty, slot);
                        if let Kind::Pointer(element) = &object.kind {
                            emitter.scale_index("x0", element);
                        }
                        emitter.text.push_str("    mov x12, x0\n");
                        emitter.atomic_rmw(width, op, order);
                        emitter.atomic_bits_to_value(object, site)
                    },
                    |emitter| {
                        emitter.atomic_runtime_transform(object, order, true, site, |emitter| {
                            emitter.push_value('x');
                            emitter.read_atomic_slot(&rhs.ty, slot);
                            emitter.combine(binary, object, &rhs.ty, object);
                        })
                    },
                )?;
            }
            ThreadFence | SignalFence | IsLockFree | IsLockFreeAt => unreachable!(),
        }
        self.adjust_stack("add", 16);
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

    // x11 holds the address. A pointer loaded from memory can name a
    // packed member, so test the address when static alignment is insufficient.
    fn atomic_access(
        &mut self,
        object: &Type,
        alignment: usize,
        inline: impl FnOnce(&mut Self, Width) -> Result<(), Diagnostic>,
        runtime: impl FnOnce(&mut Self) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        let size = object.layout().unwrap().size;
        let Some(width) = Width::of(size) else {
            return runtime(self);
        };
        if alignment >= size {
            return inline(self, width);
        }
        let slow = self.label();
        let done = self.label();
        writeln!(self.text, "    tst x11, #{}\n    b.ne {slow}", size - 1).unwrap();
        inline(self, width)?;
        writeln!(self.text, "    b {done}\n{slow}:").unwrap();
        runtime(self)?;
        writeln!(self.text, "{done}:").unwrap();
        Ok(())
    }

    fn atomic_load_at(
        &mut self,
        object: &Type,
        alignment: usize,
        order: Order,
        site: &Position,
    ) -> Result<(), Diagnostic> {
        self.atomic_access(
            object,
            alignment,
            |emitter, width| {
                let op = if order.acquire() { "ldar" } else { "ldr" };
                width.transfer(&mut emitter.text, op, 0, "x11");
                emitter.atomic_bits_to_value(object, site)
            },
            |emitter| {
                let result = emitter.frame_slot(object.layout().unwrap(), site)?;
                emitter.brt_load(object, result, order);
                emitter.read_atomic_slot(object, result);
                Ok(())
            },
        )
    }

    fn atomic_write_at(
        &mut self,
        op: AtomicOp,
        object: &Type,
        alignment: usize,
        value: usize,
        order: Order,
        site: &Position,
    ) -> Result<(), Diagnostic> {
        self.atomic_access(
            object,
            alignment,
            |emitter, width| {
                emitter.atomic_slot_address(value, "x9");
                width.transfer(&mut emitter.text, "ldr", 12, "x9");
                if op == AtomicOp::Exchange {
                    emitter.atomic_rmw(width, op, order);
                    emitter.atomic_bits_to_value(object, site)?;
                } else {
                    emitter.atomic_store(width, 12, order);
                }
                Ok(())
            },
            |emitter| {
                emitter.atomic_slot_address(value, "x2");
                if op == AtomicOp::Exchange {
                    let result = emitter.frame_slot(object.layout().unwrap(), site)?;
                    emitter.atomic_slot_address(result, "x3");
                    writeln!(emitter.text, "    mov w4, #{}", order as u8).unwrap();
                    emitter.brt_call("__atomic_exchange", object);
                    emitter.read_atomic_slot(object, result);
                } else {
                    writeln!(emitter.text, "    mov w3, #{}", order as u8).unwrap();
                    emitter.brt_call("__atomic_store", object);
                }
                Ok(())
            },
        )
    }

    fn atomic_runtime_cas(
        &mut self,
        object: &Type,
        desired: usize,
        success: Order,
        failure: Order,
        site: &Position,
    ) -> Result<(), Diagnostic> {
        let layout = object.layout().unwrap();
        let value_size = object.unqualified().layout().unwrap().size;
        let expected = if value_size != layout.size {
            // C's expected pointer names an ordinary value. Compare the
            // wider atomic representation in a private buffer.
            let slot = self.frame_slot(layout, site)?;
            self.text.push_str("    ldr x0, [sp]\n");
            self.atomic_slot_address(slot, "x9");
            self.write_atomic_value(object);
            self.text.push_str("    ldr x11, [sp, #16]\n");
            self.atomic_slot_address(slot, "x2");
            Some(slot)
        } else {
            self.text.push_str("    ldr x2, [sp]\n");
            None
        };
        self.brt_cas(object, desired, success, failure);
        if let Some(slot) = expected {
            let done = self.label();
            writeln!(self.text, "    cbnz w0, {done}").unwrap();
            self.atomic_slot_address(slot, "x0");
            self.text.push_str("    ldr x9, [sp]\n");
            self.copy_record(value_size);
            writeln!(self.text, "    mov w0, #0\n{done}:").unwrap();
        }
        Ok(())
    }

    fn atomic_runtime_transform(
        &mut self,
        object: &Type,
        order: Order,
        postfix: bool,
        site: &Position,
        compute: impl FnOnce(&mut Self),
    ) -> Result<(), Diagnostic> {
        let layout = object.layout().unwrap();
        let expected = self.frame_slot(layout, site)?;
        let desired = self.frame_slot(layout, site)?;
        self.brt_load(object, expected, Order::Relaxed);
        let retry = self.label();
        writeln!(self.text, "{retry}:").unwrap();
        self.read_atomic_slot(object, expected);
        compute(self);
        self.atomic_slot_address(desired, "x9");
        self.write_atomic_value(object);
        // CAS refreshes the snapshot on failure. The successful CAS
        // carries the requested order and retries leave the RHS alone.
        self.text.push_str("    ldr x11, [sp]\n");
        self.atomic_slot_address(expected, "x2");
        self.brt_cas(object, desired, order, Order::Relaxed);
        writeln!(self.text, "    cbz w0, {retry}").unwrap();
        self.read_atomic_slot(object, if postfix { expected } else { desired });
        Ok(())
    }

    fn brt_load(&mut self, object: &Type, result: usize, order: Order) {
        self.atomic_slot_address(result, "x2");
        writeln!(self.text, "    mov w3, #{}", order as u8).unwrap();
        self.brt_call("__atomic_load", object);
    }

    fn brt_cas(&mut self, object: &Type, desired: usize, success: Order, failure: Order) {
        self.atomic_slot_address(desired, "x3");
        writeln!(
            self.text,
            "    mov w4, #{}\n    mov w5, #{}",
            success as u8, failure as u8
        )
        .unwrap();
        self.brt_call("__atomic_compare_exchange", object);
        self.text.push_str("    uxtb w0, w0\n");
    }

    fn brt_call(&mut self, symbol: &'static str, object: &Type) {
        self.externs.insert(symbol);
        constant(&mut self.text, "x0", object.layout().unwrap().size as u64);
        writeln!(self.text, "    mov x1, x11\n    bl {symbol}").unwrap();
    }

    fn atomic_slot_address(&mut self, slot: usize, register: &str) {
        self.offset_address(register, "x29", -(slot as i64));
    }

    fn stage_atomic_value(
        &mut self,
        value: &Expression<'src>,
        object: &Type,
        site: &Position,
    ) -> Result<usize, Diagnostic> {
        let slot = self.frame_slot(object.layout().unwrap(), site)?;
        self.converted(value, object)?;
        self.atomic_slot_address(slot, "x9");
        self.write_atomic_value(object);
        Ok(slot)
    }

    fn read_atomic_slot(&mut self, object: &Type, slot: usize) {
        self.atomic_slot_address(slot, "x9");
        self.load(object, "x9");
    }

    // x9 names the destination. Copy the ordinary value and clear the
    // extra representation bytes before the runtime reads them.
    fn write_atomic_value(&mut self, object: &Type) {
        let value = object.unqualified();
        self.store_value(&value, None);
        let size = value.layout().unwrap().size;
        for byte in size..object.layout().unwrap().size {
            writeln!(self.text, "    strb wzr, [x9, #{byte}]").unwrap();
        }
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
            Kind::Record(_) | Kind::ShortVector { .. } => {
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
            Kind::Record(_) | Kind::ShortVector { .. } => {
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

fn pointer_alignment(pointer: &Expression<'_>) -> usize {
    match &pointer.kind {
        ExpressionKind::Unary(Unary::Address, place) => lvalue_alignment(place),
        _ => 1,
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
        ExpressionKind::Identifier(_)
        | ExpressionKind::Local(_)
        | ExpressionKind::ThreadLocal(_)
        | ExpressionKind::CompoundLiteral { .. }
        | ExpressionKind::StaticLiteral(_) => place.ty.layout().map_or(1, |layout| layout.align),
        _ => 1,
    }
}
