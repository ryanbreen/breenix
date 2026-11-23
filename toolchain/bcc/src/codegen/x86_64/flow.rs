//! Stack frame objects, lvalues and statement control flow.

use super::*;

#[derive(Clone)]
pub(super) enum Slot {
    Virtual(ValueId),
    Frame(usize),
    Aligned(usize),
    Static(String),
    Thread(String),
    ThreadExtern,
}

impl Slot {
    pub(super) fn memory(&self, offset: usize) -> String {
        match self {
            Self::Frame(base) => format!("[rbp - {}]", base - offset),
            Self::Aligned(base) => format!("[r15 - {}]", base - offset),
            Self::Static(name) => format!("[rip + {name}]"),
            Self::Virtual(_) => unreachable!("virtual locals use scalar accesses"),
            Self::Thread(_) | Self::ThreadExtern => {
                unreachable!("thread objects use a TP-relative slot")
            }
        }
    }
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn layout(&mut self, function: &Function<'src>) -> Result<(), Diagnostic> {
        self.locals.clear();
        self.frame = 0;
        self.aligned_frame = 0;
        self.frame_alignment = 16;
        for (index, local) in function.locals.iter().enumerate() {
            let slot = match local.storage {
                LocalStorage::ThreadExtern(_) | LocalStorage::Function(_) => Slot::ThreadExtern,
                LocalStorage::Thread(_) => Slot::Thread(self.label()),
                LocalStorage::Static(Some(_)) => Slot::Static(self.label()),
                LocalStorage::Static(None) => {
                    let layout = local.object.layout().unwrap();
                    let label = self.label();
                    writeln!(
                        self.data,
                        "    .align {}\n{label}:\n    .zero {}",
                        layout.align, layout.size
                    )
                    .unwrap();
                    Slot::Static(label)
                }
                LocalStorage::Automatic => {
                    let mut layout = local.object.layout().unwrap();
                    let ty = &local.object.ty;
                    if self.optimize
                        && matches!(ty.kind, Kind::Integer(_) | Kind::Pointer(_))
                        && !ty.qualifiers.volatile
                        && !ty.qualifiers.atomic
                    {
                        Slot::Virtual(self.scan.local(ty, layout))
                    } else {
                        // A parameter spill may store a whole register or eightbyte.
                        if index < function.params.len() {
                            layout.size = abi::eightbyte_size(layout.size, local.object.site)?;
                        }
                        self.frame_slot(layout, local.object.site)?
                    }
                }
            };
            self.locals.push(slot);
        }
        for (index, local) in function.locals.iter().enumerate() {
            match (&local.storage, &self.locals[index]) {
                (LocalStorage::Thread(init), Slot::Thread(label)) => {
                    let label = label.clone();
                    self.thread_definition(
                        &local.object,
                        &label,
                        Linkage::Internal,
                        init.as_ref(),
                    )?;
                }
                (LocalStorage::Static(Some(init)), Slot::Static(label)) => {
                    let label = label.clone();
                    let data = self.object_definition(
                        &local.object,
                        &label,
                        Linkage::Internal,
                        Some(init),
                        false,
                    )?;
                    self.initialized_data.push_str(&data);
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn frame_slot(
        &mut self,
        layout: Layout,
        site: &Position,
    ) -> Result<Slot, Diagnostic> {
        let aligned = layout.align > 16;
        let frame = if aligned {
            &mut self.aligned_frame
        } else {
            &mut self.frame
        };
        let end = abi::checked_stack_end(*frame, layout.size, site)?;
        *frame = abi::checked_stack_end(end.next_multiple_of(layout.align), 0, site)?;
        if aligned {
            self.frame_alignment = self.frame_alignment.max(layout.align);
            Ok(Slot::Aligned(*frame))
        } else {
            Ok(Slot::Frame(*frame))
        }
    }

    pub(super) fn statements(
        &mut self,
        body: &[Statement<'src>],
        result: &Type,
        epilogue: &str,
    ) -> Result<(), Diagnostic> {
        for statement in body {
            self.statement(statement, result, epilogue)?;
        }
        Ok(())
    }

    fn statement(
        &mut self,
        statement: &Statement<'src>,
        result: &Type,
        epilogue: &str,
    ) -> Result<(), Diagnostic> {
        match statement {
            Statement::Block(body) => self.statements(body, result, epilogue)?,
            Statement::Empty => {}
            Statement::Initialize { local, initializer } => self.initialize(*local, initializer)?,
            Statement::Expression(expr) => self.discard(expr)?,
            Statement::Return(expr) => {
                if let Some(expr) = expr {
                    self.converted(expr, result)?;
                }
                writeln!(self.text, "    jmp {epilogue}").unwrap();
            }
            Statement::If { condition, yes, no } => {
                let otherwise = self.label();
                self.jump_if(condition, "jz", &otherwise)?;
                self.statement(yes, result, epilogue)?;
                if let Some(no) = no {
                    let end = self.label();
                    writeln!(self.text, "    jmp {end}\n{otherwise}:").unwrap();
                    self.statement(no, result, epilogue)?;
                    writeln!(self.text, "{end}:").unwrap();
                } else {
                    writeln!(self.text, "{otherwise}:").unwrap();
                }
            }
            Statement::Loop {
                condition,
                post_test,
                increment,
                body,
                break_to,
                continue_to,
            } => {
                let loop_start = self.scan.point();
                let start = self.label();
                let next = self.flow_label(*continue_to);
                let end = self.flow_label(*break_to);
                writeln!(self.text, "{start}:").unwrap();
                if let Some(test) = condition.as_ref().filter(|_| !*post_test) {
                    self.jump_if(test, "jz", &end)?;
                }
                self.statement(body, result, epilogue)?;
                writeln!(self.text, "{next}:").unwrap();
                if let Some(increment) = increment {
                    self.discard(increment)?;
                }
                if let Some(test) = condition.as_ref().filter(|_| *post_test) {
                    self.jump_if(test, "jz", &end)?;
                }
                writeln!(self.text, "    jmp {start}\n{end}:").unwrap();
                if self.optimize {
                    self.scan.loop_back(loop_start);
                }
            }
            Statement::Switch {
                value,
                ty,
                cases,
                body,
                break_to,
            } => {
                self.converted(value, ty)?;
                let end = self.flow_label(*break_to);
                let mut default = end.clone();
                for (value, id) in cases {
                    let label = self.flow_label(*id);
                    if let Some(value) = value {
                        writeln!(
                            self.text,
                            "    mov rcx, 0x{value:x}\n    cmp rax, rcx\n    je {label}"
                        )
                        .unwrap();
                    } else {
                        default = label;
                    }
                }
                writeln!(self.text, "    jmp {default}").unwrap();
                self.statement(body, result, epilogue)?;
                writeln!(self.text, "{end}:").unwrap();
            }
            Statement::Label(id, body) => {
                if self.optimize {
                    self.scan.label(*id);
                }
                let label = self.flow_label(*id);
                writeln!(self.text, "{label}:").unwrap();
                self.statement(body, result, epilogue)?;
            }
            Statement::Jump(id) => {
                if self.optimize {
                    self.scan.jump(*id);
                }
                let label = self.flow_label(*id);
                writeln!(self.text, "    jmp {label}").unwrap();
            }
        }
        Ok(())
    }

    fn flow_label(&mut self, id: usize) -> String {
        if let Some(label) = self.flow_labels.get(&id) {
            return label.clone();
        }
        let label = self.label();
        self.flow_labels.insert(id, label.clone());
        label
    }

    pub(super) fn jump_if(
        &mut self,
        condition: &Expression<'src>,
        jump: &str,
        label: &str,
    ) -> Result<(), Diagnostic> {
        self.condition(condition)?;
        writeln!(self.text, "    test eax, eax\n    {jump} {label}").unwrap();
        Ok(())
    }

    fn initialize(
        &mut self,
        local: usize,
        initializer: &ObjectInitializer<'src>,
    ) -> Result<(), Diagnostic> {
        // N1570 6.7.9p19,21: omitted subobjects have static-storage defaults.
        if initializer.zero_bytes != 0 {
            self.zero(local, 0, initializer.zero_bytes);
        }
        for init in &initializer.values {
            match &init.value {
                InitValue::Zero => self.zero(local, init.offset, init.ty.layout().unwrap().size),
                InitValue::Expression(value) => {
                    self.converted(value, &init.ty)?;
                    if let Slot::Virtual(id) = self.locals[local] {
                        self.scalar_access(id, Access::Write);
                        continue;
                    }
                    let memory = self.locals[local].memory(init.offset);
                    if let Some(field) = init.bit_field {
                        writeln!(self.text, "    lea r11, {memory}").unwrap();
                        self.store_bit_field(field, &init.ty, None);
                    } else {
                        self.store_object(&init.ty, &memory);
                    }
                }
            }
        }
        Ok(())
    }

    fn zero(&mut self, local: usize, offset: usize, bytes: usize) {
        if let Slot::Virtual(id) = self.locals[local] {
            self.text.push_str("    xor eax, eax\n");
            self.scalar_access(id, Access::Write);
            return;
        }
        let memory = self.locals[local].memory(offset);
        writeln!(
            self.text,
            "    lea rdi, {memory}\n    mov rcx, {bytes}\n    xor eax, eax\n    rep stosb"
        )
        .unwrap();
    }

    pub(super) fn virtual_local(&self, expr: &Expression<'src>) -> Option<ValueId> {
        if let ExpressionKind::Local(local) = expr.kind {
            if let Slot::Virtual(id) = self.locals[local] {
                return Some(id);
            }
        }
        None
    }

    pub(super) fn scalar_access(&mut self, id: ValueId, access: Access) {
        self.scan.access(self.text.len(), id, access);
    }

    pub(super) fn address(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        if let Some(place) = self.thread_place(expr)? {
            // AMD64 psABI §10.3.2, Table 10.7. The Breenix TCB stores its
            // own address in FS:0.
            writeln!(
                self.text,
                "    mov rax, qword fs:[0]\n    lea rax, {}",
                place.address_operand(),
            )
            .unwrap();
            return Ok(());
        }
        if let Some(id) = self.virtual_local(expr) {
            self.scalar_access(id, Access::Address);
            return Ok(());
        }
        let memory = match &expr.kind {
            ExpressionKind::Identifier(name) => {
                self.use_symbol(name, expr.position())?;
                format!("[rip + {name}]")
            }
            ExpressionKind::Local(id) => self.locals[*id].memory(0),
            ExpressionKind::CompoundLiteral { local, initializer } => {
                self.initialize(*local, initializer)?;
                if let Slot::Virtual(id) = self.locals[*local] {
                    self.scalar_access(id, Access::Address);
                    return Ok(());
                }
                self.locals[*local].memory(0)
            }
            ExpressionKind::StaticLiteral(id) => format!("[rip + {}]", literal_label(*id)),
            ExpressionKind::String(bytes) => format!("[rip + {}]", self.emit_string(bytes)),
            ExpressionKind::Unary(Unary::Dereference, operand) => return self.value(operand),
            ExpressionKind::Member { base, offset, .. } => {
                self.value(base)?;
                if *offset != 0 {
                    // A legal member offset can exceed a signed immediate for
                    // an externally defined record.
                    if *offset <= i32::MAX as usize {
                        writeln!(self.text, "    add rax, {offset}").unwrap();
                    } else {
                        writeln!(self.text, "    mov r10, {offset}\n    add rax, r10").unwrap();
                    }
                }
                return Ok(());
            }
            _ => unreachable!("sema must supply an addressable expression"),
        };
        writeln!(self.text, "    lea rax, {memory}").unwrap();
        Ok(())
    }

    pub(super) fn assignment(
        &mut self,
        left: &Expression<'src>,
        right: &Expression<'src>,
        operation: Option<&(Binary, Type, Type)>,
    ) -> Result<(), Diagnostic> {
        if let Some(id) = self.virtual_local(left) {
            if let Some((op, a, b)) = operation {
                self.scalar_access(id, Access::Read);
                self.convert(&left.ty, a);
                let saved = self.save_operand(a);
                self.converted(right, b)?;
                self.combine(*op, a, b, a, saved);
                self.convert(a, &left.ty);
            } else {
                self.converted(right, &left.ty)?;
            }
            self.scalar_access(id, Access::Write);
            return Ok(());
        }
        if left.ty.qualifiers.atomic {
            if let Some((op, a, b)) = operation {
                return self.atomic_rmw(left, Some(right), *op, (a, b), false);
            }
            if Self::atomic_needs_storage(left) {
                return self.atomic_store_with_storage(left, right);
            }
            self.address(left)?;
            self.adjust_stack("sub", 16);
            self.text.push_str("    mov qword [rsp], rax\n");
            self.converted(right, &left.ty)?;
            self.text.push_str("    mov r11, qword [rsp]\n");
            self.atomic_assign(&left.ty);
            self.adjust_stack("add", 16);
            return Ok(());
        }
        let place = self.thread_scalar_place(left)?;
        if place.is_none() {
            self.address(left)?;
            self.adjust_stack("sub", 16);
            self.text.push_str("    mov qword [rsp], rax\n");
        }
        if let Some((op, a, b)) = operation {
            self.load_lvalue(left, place.as_ref());
            self.convert(&left.ty, a);
            let saved = self.save_operand(a);
            self.converted(right, b)?;
            self.combine(*op, a, b, a, saved);
            self.convert(a, &left.ty);
        } else {
            self.converted(right, &left.ty)?;
        }
        if left.ty.kind == Kind::LongDouble {
            self.text.push_str("    fld st(0)\n");
        }
        if place.is_none() {
            self.text.push_str("    mov r11, qword [rsp]\n");
        }
        self.store_lvalue(left, place.as_ref());
        if place.is_none() {
            self.adjust_stack("add", 16);
        }
        Ok(())
    }

    pub(super) fn update(
        &mut self,
        operand: &Expression<'src>,
        increment: bool,
        postfix: bool,
    ) -> Result<(), Diagnostic> {
        let ty = &operand.ty;
        if ty.qualifiers.atomic {
            let left = match ty.kind {
                Kind::Integer(integer) => Type::new(Kind::Integer(integer.promoted())),
                _ => ty.unqualified(),
            };
            let right = if matches!(ty.kind, Kind::Pointer(_)) {
                Type::new(Kind::Integer(Integer::Long))
            } else {
                left.clone()
            };
            let op = if increment {
                Binary::Add
            } else {
                Binary::Subtract
            };
            return self.atomic_rmw(operand, None, op, (&left, &right), postfix);
        }
        if let Some(id) = self.virtual_local(operand) {
            self.scalar_access(id, Access::Read);
            let saved = postfix.then(|| self.scan.temporary(self.text.len()));
            self.step_scalar(ty, increment, postfix);
            self.scalar_access(id, Access::Write);
            if let Some(saved) = saved {
                self.scalar_access(saved, Access::Read);
            }
            return Ok(());
        }
        let extended = ty.kind == Kind::LongDouble;
        let reserve = if postfix && extended { 32 } else { 16 };
        let place = self.thread_scalar_place(operand)?;
        if place.is_none() {
            self.address(operand)?;
        }
        self.adjust_stack("sub", reserve);
        if place.is_none() {
            self.text.push_str("    mov qword [rsp], rax\n");
        }
        self.load_lvalue(operand, place.as_ref());
        if postfix {
            if extended {
                self.text.push_str("    fld st(0)\n");
            }
            self.store(ty, "[rsp + 8]");
        }
        self.step_scalar(ty, increment, postfix);
        if place.is_none() {
            self.text.push_str("    mov r11, qword [rsp]\n");
        }
        self.store_lvalue(operand, place.as_ref());
        if postfix {
            self.load(ty, "[rsp + 8]");
        }
        self.adjust_stack("add", reserve);
        Ok(())
    }

    fn step_scalar(&mut self, ty: &Type, increment: bool, postfix: bool) {
        let op = if increment { "add" } else { "sub" };
        match &ty.kind {
            Kind::LongDouble => {
                self.text.push_str("    fld1\n");
                let op = if increment { "faddp" } else { "fsubp" };
                writeln!(self.text, "    {op} st(1), st(0)").unwrap();
                if !postfix {
                    self.text.push_str("    fld st(0)\n");
                }
            }
            Kind::Float(float) => {
                let one = match float {
                    Float::Single => 0x3f80_0000,
                    Float::Double => 0x3ff0_0000_0000_0000,
                };
                self.float_constant(one, *float, 1);
                writeln!(self.text, "    {op}{} xmm0, xmm1", suffix(*float)).unwrap();
            }
            Kind::Pointer(element) => {
                let size = element.layout().unwrap().size;
                writeln!(self.text, "    mov rcx, {size}\n    {op} rax, rcx").unwrap();
            }
            Kind::Integer(integer) => {
                writeln!(self.text, "    {op} rax, 1").unwrap();
                if *integer == Integer::Bool {
                    self.boolean(ty);
                } else {
                    self.normalize_integer(ty);
                }
            }
            _ => unreachable!(),
        }
    }

    pub(super) fn store_object(&mut self, ty: &Type, address: &str) {
        let (width, register) = match ty.kind {
            Kind::Integer(integer) => match integer.layout().size {
                1 => ("byte", "al"),
                2 => ("word", "ax"),
                4 => ("dword", "eax"),
                _ => ("qword", "rax"),
            },
            Kind::Float(float) => {
                writeln!(self.text, "    {} {address}, xmm0", float_move(float)).unwrap();
                return;
            }
            Kind::LongDouble => {
                self.store(ty, address);
                return;
            }
            Kind::Pointer(_) => ("qword", "rax"),
            Kind::Record(record) => {
                // N1570 §6.5.16.1p3 permits exact overlap for compatible
                // records. Keep the destination address as the result.
                // AMD64 psABI §3.2.1 requires DF clear at function boundaries.
                let size = record.layout.unwrap().size;
                writeln!(self.text, "    mov rsi, rax\n    lea rdi, {address}").unwrap();
                self.copy_record(size);
                // The source is an ordinary record value. Only the
                // destination has the extra bytes of the atomic type.
                for offset in 0..ty.layout().unwrap().size - size {
                    writeln!(self.text, "    mov byte ptr [rdi + {offset}], 0").unwrap();
                }
                return;
            }
            _ => unreachable!(),
        };
        writeln!(self.text, "    mov {width} {address}, {register}").unwrap();
    }

    pub(super) fn copy_record(&mut self, size: usize) {
        writeln!(
            self.text,
            "    mov rax, rdi\n    mov rcx, {size}\n    rep movsb"
        )
        .unwrap();
    }
}
