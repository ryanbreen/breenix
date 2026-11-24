//! Frame slots, lvalues and statement lowering.

use super::thread::thread_address;
use super::*;
use crate::sema::{InitValue, ObjectInitializer, StaticAddress, ThreadData, ThreadInitializer};
use crate::types::Layout;

pub(super) enum Slot {
    Frame(usize),
    Aligned(usize),
    Static(String),
    Thread(String),
    ThreadExtern,
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn layout(
        &mut self,
        function: &Function<'src>,
        parameters: &[(Option<CallObject>, ArgumentLocation)],
    ) -> Result<(), Diagnostic> {
        self.locals.clear();
        self.frame = 0;
        self.aligned_frame = 0;
        self.frame_alignment = STACK_ALIGNMENT;
        for (index, local) in function.locals.iter().enumerate() {
            let object = &local.object;
            let slot = match &local.storage {
                LocalStorage::ThreadExtern(_) | LocalStorage::Function(_) => Slot::ThreadExtern,
                LocalStorage::Thread(_) => Slot::Thread(self.label()),
                LocalStorage::Static(_) => Slot::Static(self.label()),
                LocalStorage::Automatic => {
                    let layout = match parameters.get(index) {
                        Some((Some(value), ArgumentLocation::Register(_))) if !value.indirect() => {
                            value.slot()
                        }
                        _ => object.layout().unwrap(),
                    };
                    // N1570 6.7.5: align the allocated object itself, even
                    // when the incoming stack is only 16-byte aligned.
                    if layout.align > STACK_ALIGNMENT {
                        self.frame_alignment = self.frame_alignment.max(layout.align);
                        self.aligned_frame = next_frame_slot(self.aligned_frame, layout, object.site)?;
                        Slot::Aligned(self.aligned_frame)
                    } else {
                        Slot::Frame(self.frame_slot(layout, object.site)?)
                    }
                }
            };
            self.locals.push(slot);
        }
        for (index, local) in function.locals.iter().enumerate() {
            if let (LocalStorage::Thread(init), Slot::Thread(label)) =
                (&local.storage, &self.locals[index])
            {
                let label = label.clone();
                self.thread_definition(&local.object, &label, Linkage::Internal, init.as_ref())?;
            }
            if let (LocalStorage::Static(init), Slot::Static(label)) =
                (&local.storage, &self.locals[index])
            {
                let label = label.clone();
                self.object_definition(&local.object, &label, Linkage::Internal, init.as_ref())?;
            }
        }
        Ok(())
    }

    pub(super) fn frame_slot(&mut self, layout: Layout, site: &Position) -> Result<usize, Diagnostic> {
        self.frame = next_frame_slot(self.frame, layout, site)?;
        Ok(self.frame)
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

    fn inline_asm(&mut self, asm: &crate::sema::InlineAsm<'src>) -> Result<(), Diagnostic> {
        for (index, operand) in asm.registers.iter().enumerate().filter(|(_, operand)| operand.input) {
            self.address(&operand.slot)?;
            let register = crate::sema::asm_register(Target::Aarch64, index, 8);
            writeln!(self.text, "    ldr {register}, [x0]").unwrap();
        }
        writeln!(self.text, "{}", asm.template).unwrap();
        for (index, operand) in asm.registers.iter().enumerate().filter(|(_, operand)| operand.output) {
            self.address(&operand.slot)?;
            let register = crate::sema::asm_register(Target::Aarch64, index, 8);
            writeln!(self.text, "    str {register}, [x0]").unwrap();
        }
        Ok(())
    }

    fn vla_stack(&mut self, mark: usize, restore: bool) {
        self.local_address(mark, 0, "x0");
        let base = self.alloca_base.unwrap();
        self.offset_address("x9", "x29", -(base as i64));
        if restore {
            self.text.push_str("    ldr x10, [x0]\n    mov sp, x10\n    str x10, [x9]\n");
        } else {
            self.text.push_str("    ldr x10, [x9]\n    str x10, [x0]\n");
        }
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
            Statement::InlineAsm(asm) => self.inline_asm(asm)?,
            Statement::Initialize { local, initializer } => {
                self.initialize(*local, initializer)?;
            }
            Statement::Expression(expr) => self.value(expr)?,
            Statement::Return(expr) => {
                if let Some(expr) = expr {
                    self.converted(expr, result)?;
                }
                writeln!(self.text, "    b {epilogue}").unwrap();
            }
            Statement::If { condition, yes, no } => {
                let otherwise = self.label();
                self.jump_if(condition, false, &otherwise)?;
                self.statement(yes, result, epilogue)?;
                if let Some(no) = no {
                    let end = self.label();
                    writeln!(self.text, "    b {end}\n{otherwise}:").unwrap();
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
                let start = self.label();
                let next = self.flow_label(*continue_to);
                let end = self.flow_label(*break_to);
                writeln!(self.text, "{start}:").unwrap();
                if let Some(test) = condition.as_ref().filter(|_| !*post_test) {
                    self.jump_if(test, false, &end)?;
                }
                self.statement(body, result, epilogue)?;
                writeln!(self.text, "{next}:").unwrap();
                if let Some(increment) = increment {
                    self.value(increment)?;
                }
                if let Some(test) = condition.as_ref().filter(|_| *post_test) {
                    self.jump_if(test, true, &start)?;
                } else {
                    writeln!(self.text, "    b {start}").unwrap();
                }
                writeln!(self.text, "{end}:").unwrap();
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
                    if let Some(crate::sema::CaseRange { low, high }) = value {
                        constant(&mut self.text, "x9", *low);
                        if low == high {
                            writeln!(self.text, "    cmp x0, x9\n    b.eq {label}").unwrap();
                        } else {
                            let next = self.label();
                            let signed = matches!(ty.kind, Kind::Integer(integer)
                                if integer.is_signed(Target::Aarch64));
                            let (below, within) = if signed { ("lt", "le") } else { ("lo", "ls") };
                            writeln!(self.text, "    cmp x0, x9\n    b.{below} {next}").unwrap();
                            constant(&mut self.text, "x9", *high);
                            writeln!(self.text, "    cmp x0, x9\n    b.{within} {label}\n{next}:")
                                .unwrap();
                        }
                    } else {
                        default = label;
                    }
                }
                writeln!(self.text, "    b {default}").unwrap();
                self.statement(body, result, epilogue)?;
                writeln!(self.text, "{end}:").unwrap();
            }
            Statement::Label(id, body) => {
                let label = self.flow_label(*id);
                writeln!(self.text, "{label}:").unwrap();
                self.statement(body, result, epilogue)?;
            }
            Statement::SaveVlaStack(mark) => self.vla_stack(*mark, false),
            Statement::RestoreVlaStack(mark) => self.vla_stack(*mark, true),
            Statement::Jump(id, scopes) => {
                let depth = self.vla_targets.get(id).copied().unwrap_or(0);
                if let Some(&mark) = scopes.get(depth) {
                    self.vla_stack(mark, true);
                }
                self.restore_statement_stack(*id);
                let label = self.flow_label(*id);
                writeln!(self.text, "    b {label}").unwrap();
            }
        }
        Ok(())
    }

    pub(super) fn statement_expression(
        &mut self,
        region: usize,
        statements: &[Statement<'src>],
        value: Option<&Expression<'src>>,
        expr: &Expression<'src>,
    ) -> Result<(), Diagnostic> {
        // GNU C §6.12.1 permits jumps out of a statement expression.
        // Record live spill depth separately from the base moved by alloca.
        let slot = self.frame_slot(Layout { size: 8, align: 8 }, expr.position())?;
        let base = self.statement_base.unwrap();
        self.offset_address("x9", "x29", -(base as i64));
        self.text.push_str("    ldr x10, [x9]\n    mov x11, sp\n    sub x11, x11, x10\n");
        self.offset_address("x9", "x29", -(slot as i64));
        self.text.push_str("    str x11, [x9]\n");

        self.statement_stack.push((region, slot));
        let (function, epilogue) = self.return_context.as_ref().unwrap();
        let (function, epilogue) = (*function, epilogue.clone());
        let Kind::Function { result, .. } = &function.ty.kind else { unreachable!() };
        self.statements(statements, result, &epilogue)?;
        if let Some(value) = value {
            self.converted(value, &expr.ty)?;
        }
        self.statement_stack.pop();
        Ok(())
    }

    fn restore_statement_stack(&mut self, id: usize) {
        let Some((current, _)) = self.statement_stack.last() else { return };
        let (function, _) = self.return_context.as_ref().unwrap();
        let target = function.jump_regions.get(&id);
        if target == Some(current) {
            return;
        }

        let base = self.statement_base.unwrap();
        self.offset_address("x9", "x29", -(base as i64));
        self.text.push_str("    ldr x10, [x9]\n");
        if let Some(target) = target {
            let (_, slot) = self.statement_stack.iter().find(|(region, _)| region == target).unwrap();
            self.offset_address("x9", "x29", -(*slot as i64));
            self.text.push_str("    ldr x11, [x9]\n    add x10, x10, x11\n");
        }
        self.text.push_str("    mov sp, x10\n");
    }

    fn initialize(
        &mut self,
        local: usize,
        initializer: &ObjectInitializer<'src>,
    ) -> Result<(), Diagnostic> {
        self.zero_local(local, 0, initializer.zero_bytes);
        for init in &initializer.values {
            let InitValue::Expression(value) = &init.value else {
                self.zero_local(local, init.offset, init.ty.layout().unwrap().size);
                continue;
            };
            self.converted(value, &init.ty)?;
            if init.ty.qualifiers.atomic && matches!(init.ty.kind, Kind::Record(_)) {
                self.atomic_record_initialize(&init.ty, local, init.offset);
            } else {
                self.local_address(local, init.offset, "x9");
                self.store_value(&init.ty, init.bit_field);
            }
        }
        Ok(())
    }

    fn zero_local(&mut self, local: usize, offset: usize, len: usize) {
        if len == 0 {
            return;
        }
        self.local_address(local, offset, "x9");
        constant(&mut self.text, "x10", len as u64);
        let clear = self.label();
        writeln!(
            self.text,
            "{clear}:\n    strb wzr, [x9], #1\n    subs x10, x10, #1\n    b.ne {clear}"
        )
        .unwrap();
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
        branch_on_true: bool,
        label: &str,
    ) -> Result<(), Diagnostic> {
        self.value(condition)?;
        if matches!(condition.ty.kind, Kind::Float(_) | Kind::LongDouble) {
            self.compare_zero(&condition.ty);
            let condition = if branch_on_true { "ne" } else { "eq" };
            writeln!(self.text, "    b.{condition} {label}").unwrap();
        } else {
            let jump = if branch_on_true { "cbnz" } else { "cbz" };
            writeln!(self.text, "    {jump} x0, {label}").unwrap();
        }
        Ok(())
    }

    pub(super) fn address(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        match &expr.kind {
            ExpressionKind::Identifier(name) => {
                self.use_symbol(name, expr.position())?;
                symbol_address(&mut self.text, "x0", name);
            }
            ExpressionKind::Local(id) => self.local_address(*id, 0, "x0"),
            ExpressionKind::CompoundLiteral { local, initializer } => {
                self.initialize(*local, initializer)?;
                self.local_address(*local, 0, "x0");
            }
            ExpressionKind::StaticLiteral(id) => {
                symbol_address(&mut self.text, "x0", &literal_label(*id));
            }
            ExpressionKind::String(bytes) => {
                let label = self.emit_string(bytes);
                symbol_address(&mut self.text, "x0", &label);
            }
            ExpressionKind::Unary(Unary::Dereference, operand) => self.value(operand)?,
            ExpressionKind::Member { base, offset, .. } => {
                self.value(base)?;
                self.add_sub_offset("add", "x0", "x0", *offset as u64);
            }
            ExpressionKind::ThreadLocal(name) => {
                check_symbol(name, expr.position())?;
                if !self.thread_definitions.contains(name) {
                    self.thread_externs.insert(name);
                }
                thread_address(&mut self.text, "x0", name);
            }
            _ => unreachable!("sema must supply an addressable expression"),
        }
        Ok(())
    }

    pub(super) fn local_address(&mut self, id: usize, offset: usize, register: &str) {
        match &self.locals[id] {
            Slot::Frame(distance) => {
                let displacement = -((distance - offset) as i64);
                self.offset_address(register, "x29", displacement);
            }
            Slot::Aligned(distance) => {
                let displacement = -((distance - offset) as i64);
                self.offset_address(register, "x19", displacement);
            }
            Slot::Static(name) => {
                symbol_address(&mut self.text, register, name);
                if offset != 0 {
                    self.offset_address(register, register, offset as i64);
                }
            }
            Slot::Thread(name) => {
                thread_address(&mut self.text, register, name);
                if offset != 0 {
                    self.offset_address(register, register, offset as i64);
                }
            }
            Slot::ThreadExtern => unreachable!("sema uses a named TLS reference"),
        }
    }

    pub(super) fn assignment(
        &mut self,
        left: &Expression<'src>,
        right: &Expression<'src>,
        operation: Option<&(Binary, Type, Type)>,
    ) -> Result<(), Diagnostic> {
        if left.ty.qualifiers.atomic {
            return self.atomic_assignment(left, right, operation);
        }
        self.address(left)?;
        let field = left.bit_field();
        if operation.is_none() && matches!(&right.kind, ExpressionKind::Call { callee, .. }
            if matches!(&callee.kind, ExpressionKind::Identifier(name)
                if self.functions.get(name).is_some_and(|function| function.returns_twice)))
        {
            // A later call can overwrite an SP-relative expression spill
            // before the returns-twice call resumes. Keep the destination
            // address within the live frame until either return stores it.
            let slot = self.frame_slot(Layout { size: 8, align: 8 }, left.position())?;
            self.offset_address("x9", "x29", -(slot as i64));
            self.text.push_str("    str x0, [x9]\n");
            self.converted(right, &left.ty)?;
            self.offset_address("x9", "x29", -(slot as i64));
            self.text.push_str("    ldr x9, [x9]\n");
            self.store_value(&left.ty, field);
            return Ok(());
        }
        self.push_value('x');
        if let Some((op, a, b)) = operation {
            self.load_value(&left.ty, field, "x0");
            self.convert(&left.ty, a);
            self.push_value(value_prefix(a));
            self.converted(right, b)?;
            self.combine(*op, a, b, a);
            self.convert(a, &left.ty);
        } else {
            self.converted(right, &left.ty)?;
        }
        self.text.push_str("    ldr x9, [sp], #16\n");
        self.store_value(&left.ty, field);
        Ok(())
    }

    pub(super) fn update(
        &mut self,
        operand: &Expression<'src>,
        increment: bool,
        postfix: bool,
    ) -> Result<(), Diagnostic> {
        if operand.ty.qualifiers.atomic {
            return self.atomic_update(operand, increment, postfix);
        }
        self.address(operand)?;
        self.text.push_str("    mov x9, x0\n");
        let ty = &operand.ty;
        let field = operand.bit_field();
        self.load_value(ty, field, "x9");
        if ty.kind == Kind::LongDouble {
            self.binary128_update(increment, postfix);
            return Ok(());
        }
        if let Kind::Float(precision) = ty.kind {
            self.float_update(precision, increment, postfix);
            return Ok(());
        }
        if postfix {
            self.text.push_str("    mov x10, x0\n");
        }
        let op = if increment { "add" } else { "sub" };
        let step = match &ty.kind {
            Kind::Pointer(element) => element.layout().unwrap().size,
            _ => 1,
        };
        self.add_sub_offset(op, "x0", "x0", step as u64);
        if ty.kind == Kind::Integer(Integer::Bool) {
            self.boolean(ty);
        } else {
            self.normalize(ty);
        }
        self.store_value(ty, field);
        if postfix {
            self.text.push_str("    mov x0, x10\n");
        }
        Ok(())
    }

    pub(super) fn emit_string(&mut self, bytes: &[u8]) -> String {
        let label = self.label();
        writeln!(self.rodata, "{label}:").unwrap();
        for chunk in bytes.chunks(16) {
            self.rodata.push_str("    .ascii \"");
            for byte in chunk {
                write!(self.rodata, "\\x{byte:02x}").unwrap();
            }
            self.rodata.push_str("\"\n");
        }
        label
    }

    // N1570 6.7.9 and 6.9.2: static initializers supply bytes and
    // addresses, and definitions without an initializer are zero-initialized.
    pub(super) fn object_definition(
        &mut self,
        object: &Object<'_>,
        name: &str,
        linkage: Linkage,
        initializer: Option<&ThreadInitializer<'src>>,
    ) -> Result<(), Diagnostic> {
        let data = self.object_data(object, name, linkage, initializer)?;
        self.push_object_data(initializer.is_some(), &data);
        Ok(())
    }

    pub(super) fn object_data(
        &mut self,
        object: &Object<'_>,
        name: &str,
        linkage: Linkage,
        initializer: Option<&ThreadInitializer<'src>>,
    ) -> Result<String, Diagnostic> {
        check_symbol(name, object.site)?;
        let layout = object.layout().unwrap();
        let mut data = String::new();
        if linkage == Linkage::External {
            writeln!(data, ".global {name}").unwrap();
        }
        writeln!(
            data,
            ".type {name}, @object\n    .balign {}\n{name}:",
            layout.align
        )
        .unwrap();
        self.emit_initializer(&mut data, initializer, layout.size)?;
        writeln!(data, ".size {name}, {}", layout.size).unwrap();
        Ok(data)
    }

    pub(super) fn push_object_data(&mut self, initialized: bool, data: &str) {
        if initialized {
            self.data.push_str(data);
        } else {
            self.bss.push_str(data);
        }
    }

    // N1570 6.6p7,9 and 6.7.9p4: static and TLS initializers both
    // admit address constants. AAELF64 §5.7.5 defines the ABS64 relocation.
    pub(super) fn emit_initializer(
        &mut self,
        data: &mut String,
        initializer: Option<&ThreadInitializer<'src>>,
        size: usize,
    ) -> Result<(), Diagnostic> {
        let mut cursor = 0;
        for (&offset, value) in initializer.into_iter().flatten() {
            if offset > cursor {
                writeln!(data, "    .zero {}", offset - cursor).unwrap();
            }
            cursor = offset + match value {
                ThreadData::Byte(byte) => {
                    writeln!(data, "    .byte {byte}").unwrap();
                    1
                }
                ThreadData::Address(address, addend) => {
                    let target = match address {
                        StaticAddress::Symbol(symbol, site) => {
                            self.use_symbol(symbol, site)?;
                            (*symbol).to_owned()
                        }
                        StaticAddress::String(bytes) => self.emit_string(bytes),
                        StaticAddress::Literal(id) => literal_label(*id),
                        StaticAddress::Local(index) => {
                            let Slot::Static(label) = &self.locals[*index] else {
                                unreachable!("initializer addresses name ordinary static locals");
                            };
                            label.clone()
                        }
                    };
                    writeln!(data, "    .quad {target}{addend:+}").unwrap();
                    8
                }
            };
        }
        if cursor < size {
            writeln!(data, "    .zero {}", size - cursor).unwrap();
        }
        Ok(())
    }
}

fn next_frame_slot(frame: usize, layout: Layout, site: &Position) -> Result<usize, Diagnostic> {
    frame
        .checked_add(layout.size)
        .and_then(|end| end.checked_next_multiple_of(layout.align))
        .filter(|&end| end <= i32::MAX as usize - (STACK_ALIGNMENT - 1))
        .ok_or_else(|| Diagnostic::new(site, "stack frame exceeds the addressable range"))
}

pub(super) fn literal_label(id: usize) -> String {
    format!(".L.init.{id}")
}

pub(super) fn symbol_address(out: &mut String, register: &str, name: &str) {
    writeln!(
        out,
        "    adrp {register}, {name}\n    add {register}, {register}, :lo12:{name}"
    )
    .unwrap();
}
