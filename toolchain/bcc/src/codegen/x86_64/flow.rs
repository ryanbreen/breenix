//! Stack frame objects, lvalues and statement control flow.

use super::*;

pub(super) enum Slot {
    Frame(usize),
    Static(String),
}

impl Slot {
    pub(super) fn memory(&self, offset: usize) -> String {
        match self {
            Self::Frame(base) => format!("[rbp - {}]", base - offset),
            Self::Static(name) => format!("[rip + {name}]"),
        }
    }
}

impl<'src> Emitter<'_, 'src> {
    pub(super) fn layout(&mut self, function: &Function<'src>) -> Result<usize, Diagnostic> {
        self.locals.clear();
        let mut frame = 0usize;
        for (index, local) in function.locals.iter().enumerate() {
            let layout = local.object.ty.layout().unwrap();
            let slot = if local.static_storage {
                let label = self.label();
                writeln!(
                    self.data,
                    "    .align {}\n{label}:\n    .zero {}",
                    layout.align, layout.size
                )
                .unwrap();
                Slot::Static(label)
            } else {
                // A parameter spill may store a whole register or eightbyte.
                let size = if index < function.params.len() {
                    8
                } else {
                    layout.size
                };
                frame = frame
                    .checked_add(size)
                    .and_then(|frame| frame.checked_next_multiple_of(layout.align))
                    .filter(|&frame| frame <= i32::MAX as usize - 15)
                    .ok_or_else(|| {
                        Diagnostic::new(
                            local.object.site,
                            "stack frame exceeds the addressable range",
                        )
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
            Statement::Initialize {
                local,
                values,
                zero_bytes,
            } => {
                // N1570 6.7.9p21: elements omitted from an aggregate
                // initializer are initialized as if they had static storage.
                if *zero_bytes != 0 {
                    let memory = self.locals[*local].memory(0);
                    writeln!(self.text, "    lea rdi, {memory}\n    mov rcx, {zero_bytes}\n    xor eax, eax\n    rep stosb").unwrap();
                }
                for init in values {
                    self.converted(&init.value, &init.ty)?;
                    let memory = self.locals[*local].memory(init.offset);
                    self.store_object(&init.ty, &memory);
                }
            }
            Statement::Expression(expr) => self.value(expr)?,
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
                    self.value(increment)?;
                }
                if let Some(test) = condition.as_ref().filter(|_| *post_test) {
                    self.jump_if(test, "jz", &end)?;
                }
                writeln!(self.text, "    jmp {start}\n{end}:").unwrap();
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
                let label = self.flow_label(*id);
                writeln!(self.text, "{label}:").unwrap();
                self.statement(body, result, epilogue)?;
            }
            Statement::Jump(id) => {
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

    pub(super) fn address(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        let memory = match &expr.kind {
            ExpressionKind::Identifier(name) => {
                self.use_symbol(name, expr.position())?;
                format!("[rip + {name}]")
            }
            ExpressionKind::Local(id) => self.locals[*id].memory(0),
            ExpressionKind::String(bytes) => format!("[rip + {}]", self.emit_string(bytes)),
            ExpressionKind::Unary(Unary::Dereference, operand) => return self.value(operand),
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
        self.address(left)?;
        self.adjust_stack("sub", 16);
        self.text.push_str("    mov qword [rsp], rax\n");
        if let Some((op, a, b)) = operation {
            self.load(&left.ty, "[rax]");
            self.convert(&left.ty, a);
            self.adjust_stack("sub", 16);
            self.store(a, "[rsp]");
            self.converted(right, b)?;
            self.combine(*op, a, b, a);
            self.convert(a, &left.ty);
        } else {
            self.converted(right, &left.ty)?;
        }
        self.text.push_str("    mov r11, qword [rsp]\n");
        self.store_object(&left.ty, "[r11]");
        self.adjust_stack("add", 16);
        Ok(())
    }

    pub(super) fn update(
        &mut self,
        operand: &Expression<'src>,
        increment: bool,
        postfix: bool,
    ) -> Result<(), Diagnostic> {
        self.address(operand)?;
        self.adjust_stack("sub", 16);
        self.text.push_str("    mov qword [rsp], rax\n");
        let ty = &operand.ty;
        self.load(ty, "[rax]");
        if postfix {
            self.store(ty, "[rsp + 8]");
        }
        let op = if increment { "add" } else { "sub" };
        match &ty.kind {
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
        self.text.push_str("    mov r11, qword [rsp]\n");
        self.store_object(ty, "[r11]");
        if postfix {
            self.load(ty, "[rsp + 8]");
        }
        self.adjust_stack("add", 16);
        Ok(())
    }

    fn store_object(&mut self, ty: &Type, address: &str) {
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
            Kind::Pointer(_) => ("qword", "rax"),
            _ => unreachable!(),
        };
        writeln!(self.text, "    mov {width} {address}, {register}").unwrap();
    }
}
