//! Frame slots, lvalues and statement lowering.

use super::*;

pub(super) enum Slot {
    Frame(usize),
    Static(String),
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn layout(&mut self, function: &Function<'src>) -> Result<usize, Diagnostic> {
        self.locals.clear();
        let mut frame = 0usize;
        for local in &function.locals {
            let object = &local.object;
            require_nonatomic(&object.ty, object.site)?;
            let layout = object.ty.layout().unwrap();
            if matches!(local.storage, LocalStorage::Thread(_) | LocalStorage::ThreadExtern(_)) {
                return Err(Diagnostic::new(object.site, "AArch64 C TLS is deferred to A4"));
            }
            if matches!(local.storage, LocalStorage::Static(Some(_))) || object.alignment.is_some() {
                return Err(Diagnostic::new(object.site, "AArch64 C11 object lowering is deferred to 25.9"));
            }
            let slot = if matches!(local.storage, LocalStorage::Static(None)) {
                let label = self.label();
                writeln!(
                    self.bss,
                    "    .balign {}\n{label}:\n    .zero {}",
                    layout.align, layout.size
                )
                .unwrap();
                Slot::Static(label)
            } else {
                frame = frame
                    .checked_add(layout.size)
                    .and_then(|end| end.checked_next_multiple_of(layout.align))
                    .filter(|&end| end <= i32::MAX as usize - 15)
                    .ok_or_else(|| {
                        Diagnostic::new(object.site, "stack frame exceeds the addressable range")
                    })?;
                Slot::Frame(frame)
            };
            self.locals.push(slot);
        }
        Ok(frame.next_multiple_of(16))
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
            Statement::Initialize { local, initializer } => {
                self.zero_local(*local, 0, initializer.zero_bytes);
                for init in &initializer.values {
                    let InitValue::Expression(value) = &init.value else {
                        self.zero_local(*local, init.offset, init.ty.layout().unwrap().size);
                        continue;
                    };
                    if init.bit_field.is_some() {
                        return Err(Diagnostic::new(value.position(), DEFERRED_A4));
                    }
                    self.converted(value, &init.ty)?;
                    self.local_address(*local, init.offset, "x9");
                    self.store_object(&init.ty, 0, "x9");
                }
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
                    if let Some(value) = value {
                        constant(&mut self.text, "x9", *value);
                        writeln!(self.text, "    cmp x0, x9\n    b.eq {label}").unwrap();
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
            Statement::Jump(id) => {
                let label = self.flow_label(*id);
                writeln!(self.text, "    b {label}").unwrap();
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
        if matches!(condition.ty.kind, Kind::Float(_)) {
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
            ExpressionKind::String(bytes) => {
                let label = self.emit_string(bytes);
                symbol_address(&mut self.text, "x0", &label);
            }
            ExpressionKind::Unary(Unary::Dereference, operand) => self.value(operand)?,
            ExpressionKind::ThreadLocal(_) | ExpressionKind::Member { .. } => {
                return Err(Diagnostic::new(expr.position(), DEFERRED_A4));
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
            Slot::Static(name) => {
                symbol_address(&mut self.text, register, name);
                if offset != 0 {
                    self.offset_address(register, register, offset as i64);
                }
            }
        }
    }

    pub(super) fn assignment(
        &mut self,
        left: &Expression<'src>,
        right: &Expression<'src>,
        operation: Option<&(Binary, Type, Type)>,
    ) -> Result<(), Diagnostic> {
        require_nonatomic(&left.ty, left.position())?;
        self.address(left)?;
        self.push_value('x');
        if let Some((op, a, b)) = operation {
            self.load(&left.ty, "x0");
            self.convert(&left.ty, a);
            self.push_value(value_prefix(a));
            self.converted(right, b)?;
            self.combine(*op, a, b, a);
            self.convert(a, &left.ty);
        } else {
            self.converted(right, &left.ty)?;
        }
        self.text.push_str("    ldr x9, [sp], #16\n");
        self.store_object(&left.ty, 0, "x9");
        Ok(())
    }

    pub(super) fn update(
        &mut self,
        operand: &Expression<'src>,
        increment: bool,
        postfix: bool,
    ) -> Result<(), Diagnostic> {
        require_nonatomic(&operand.ty, operand.position())?;
        self.address(operand)?;
        self.text.push_str("    mov x9, x0\n");
        let ty = &operand.ty;
        self.load(ty, "x9");
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
        self.store_object(ty, 0, "x9");
        if postfix {
            self.text.push_str("    mov x0, x10\n");
        }
        Ok(())
    }

    fn emit_string(&mut self, bytes: &[u8]) -> String {
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

    // N1570 6.7.9 and 6.9.2: file-scope initializers supply static bytes
    // and addresses, and tentative definitions end up zero-initialized.
    pub(super) fn object_definition(
        &mut self,
        definition: &StaticObject<'src>,
    ) -> Result<(), Diagnostic> {
        let object = &definition.object;
        let name = object.name;
        check_symbol(name, object.site)?;
        let layout = object.layout().unwrap();
        let mut data = String::new();
        if definition.linkage == Linkage::External {
            writeln!(data, ".global {name}").unwrap();
        }
        writeln!(
            data,
            ".type {name}, @object\n    .balign {}\n{name}:",
            layout.align
        )
        .unwrap();
        let mut cursor = 0;
        if let Some(initializer) = &definition.initializer {
            require_nonatomic(&object.ty, object.site)?;
            for (&offset, value) in initializer {
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
                            StaticAddress::Literal(_) | StaticAddress::Local(_) => {
                                return Err(Diagnostic::new(
                                    object.site,
                                    "AArch64 static initializer address is deferred to ARM 25.9d",
                                ));
                            }
                        };
                        writeln!(data, "    .quad {target}{addend:+}").unwrap();
                        8
                    }
                };
            }
        }
        if cursor < layout.size {
            writeln!(data, "    .zero {}", layout.size - cursor).unwrap();
        }
        writeln!(data, ".size {name}, {}", layout.size).unwrap();
        if definition.initializer.is_some() {
            self.data.push_str(&data);
        } else {
            self.bss.push_str(&data);
        }
        Ok(())
    }
}

fn symbol_address(out: &mut String, register: &str, name: &str) {
    writeln!(
        out,
        "    adrp {register}, {name}\n    add {register}, {register}, :lo12:{name}"
    )
    .unwrap();
}
