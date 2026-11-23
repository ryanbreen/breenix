//! Unoptimized AArch64 assembly for bas.
//!
//! AAPCS64 2025Q1 §§6.1, 6.4.5–6.4.6, 6.8–6.9 and §10.1.1 Table 3
//! define the register, stack and C type conventions. A64 instructions
//! and their operand forms follow Arm DDI 0487 M.b Chapters C6–C7.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use crate::pp::{Diagnostic, Position, Target};
use crate::sema::{
    Argument, Binary, Expression, ExpressionKind, Function, Linkage, LocalStorage, Object,
    Statement, TranslationUnit, Unary,
};
use crate::types::{Float, Integer, Kind, Layout, Record, Type};

mod abi;
mod atomic;
mod binary128;
mod builtins;
mod float;
mod flow;
mod record;
mod thread;
mod varargs;

use abi::{ArgumentLocation, ArgumentSlots, CallObject};
use flow::Slot;
use varargs::VariadicFrame;

const ARGUMENT_REGISTERS: usize = 8;
const FRAME_RECORD: usize = 16;
const STACK_ALIGNMENT: usize = 16;

pub(super) fn emit(unit: &TranslationUnit<'_>, options: super::Options) -> Result<String, Diagnostic> {
    let mut functions = HashMap::new();
    for function in &unit.functions {
        let entry = functions.entry(function.name).or_insert(function);
        if function.body.is_some() {
            *entry = function;
        }
    }
    let mut custom_data = String::new();
    let mut emitter = Emitter {
        functions,
        records: &unit.records,
        locals: Vec::new(),
        frame: 0,
        aligned_frame: 0,
        frame_alignment: STACK_ALIGNMENT,
        alloca_base: None,
        statement_base: None,
        statement_stack: Vec::new(),
        return_context: None,
        variadic: None,
        flow_labels: HashMap::new(),
        vla_targets: HashMap::new(),
        object_definitions: unit
            .static_objects
            .iter()
            .map(|def| def.object.name)
            .chain(unit.aliases.keys().copied())
            .collect(),
        externs: BTreeSet::new(),
        thread_externs: BTreeSet::new(),
        thread_definitions: unit
            .thread_objects
            .iter()
            .map(|thread| thread.object.name)
            .chain(unit.aliases.keys().copied())
            .collect(),
        text: String::from(".section .text\n"),
        rodata: String::new(),
        data: String::new(),
        bss: String::new(),
        tdata: String::new(),
        tbss: String::new(),
        next_label: 0,
    };
    // N1570 6.5.2.5p5: file-scope compound literals have static storage.
    for (id, literal) in unit.static_literals.iter().enumerate() {
        let label = flow::literal_label(id);
        let object = Object {
            name: &label,
            site: literal.site,
            ty: literal.ty.clone(),
            alignment: None,
        };
        emitter.object_definition(
            &object, &label, Linkage::Internal, Some(&literal.initializer),
        )?;
    }
    for definition in &unit.static_objects {
        let object = &definition.object;
        let section = unit.symbol_attributes.get(object.name).and_then(|attrs| attrs.section.as_deref());
        let data = emitter.object_data(
            object, object.name, definition.linkage, definition.initializer.as_ref(),
        )?;
        if let Some(section) = section {
            super::object_section(&mut custom_data, section, object, false);
            custom_data.push_str(&data);
        } else {
            emitter.push_object_data(definition.initializer.is_some(), &data);
        }
    }
    for thread in &unit.thread_objects {
        let object = &thread.object;
        if let Some(section) = unit.symbol_attributes.get(object.name).and_then(|attrs| attrs.section.as_deref()) {
            super::object_section(&mut custom_data, section, object, true);
            let data = emitter.thread_data(
                object, object.name, thread.linkage, thread.initializer.as_ref(),
            )?;
            custom_data.push_str(&data);
        } else {
            emitter.thread_definition(
                object, object.name, thread.linkage, thread.initializer.as_ref(),
            )?;
        }
    }
    for function in &unit.functions {
        if let Some(body) = &function.body {
            let custom_section = super::function_section(
                &mut emitter.text, unit, function.name, options.function_sections,
            );
            emitter.function(function, body)?;
            if custom_section {
                emitter.text.push_str("\n.section .text\n");
            }
        }
    }
    for name in &emitter.externs {
        writeln!(emitter.text, ".extern {name}").unwrap();
    }
    for name in &emitter.thread_externs {
        writeln!(emitter.text, ".extern {name}\n.type {name}, @tls").unwrap();
    }
    if !emitter.rodata.is_empty() {
        emitter.text.push_str("\n.section .rodata\n");
        emitter.text.push_str(&emitter.rodata);
    }
    for (section, body) in [
        (".data", emitter.data),
        (".bss", emitter.bss),
        (".tdata", emitter.tdata),
        (".tbss", emitter.tbss),
    ] {
        if !body.is_empty() {
            writeln!(emitter.text, "\n.section {section}").unwrap();
            emitter.text.push_str(&body);
        }
    }
    emitter.text.push_str(&custom_data);
    super::emit_symbol_attributes(&mut emitter.text, unit, &emitter.externs, &emitter.thread_externs);
    Ok(emitter.text)
}

struct Emitter<'unit, 'src> {
    functions: HashMap<&'src str, &'unit Function<'src>>,
    records: &'unit [Record<'src>],
    locals: Vec<Slot>,
    frame: usize,
    aligned_frame: usize,
    frame_alignment: usize,
    alloca_base: Option<usize>,
    statement_base: Option<usize>,
    statement_stack: Vec<(usize, usize)>,
    return_context: Option<(&'unit Function<'src>, String)>,
    variadic: Option<VariadicFrame>,
    flow_labels: HashMap<usize, String>,
    vla_targets: HashMap<usize, usize>,
    object_definitions: BTreeSet<&'src str>,
    externs: BTreeSet<&'src str>,
    thread_externs: BTreeSet<&'src str>,
    thread_definitions: BTreeSet<&'src str>,
    text: String,
    rodata: String,
    data: String,
    bss: String,
    tdata: String,
    tbss: String,
    next_label: usize,
}

impl<'unit, 'src> Emitter<'unit, 'src> {
    fn function(
        &mut self,
        function: &'unit Function<'src>,
        body: &[Statement<'src>],
    ) -> Result<(), Diagnostic> {
        let name = function.name;
        check_symbol(name, function.site)?;
        let Kind::Function {
            result, params: abi_params, variadic, ..
        } = &function.ty.kind
        else {
            unreachable!()
        };
        let object_result = self.call_object(result, function.site)?;
        let mut slots = ArgumentSlots::default();
        let mut parameters = Vec::with_capacity(function.params.len());
        for (param, incoming) in function.params.iter().zip(abi_params) {
            let object = self.call_object(incoming, param.name.unwrap().1)?;
            parameters.push((object, slots.next(incoming, object)));
        }
        self.layout(function, &parameters)?;
        self.alloca_base = if function.has_alloca {
            Some(self.frame_slot(Layout { size: 8, align: 8 }, function.site)?)
        } else {
            None
        };
        self.statement_base = match (function.has_statement_expressions, self.alloca_base) {
            (true, None) => Some(self.frame_slot(Layout { size: 8, align: 8 }, function.site)?),
            (_, base) => base,
        };
        let indirect_result = if object_result.is_some_and(CallObject::indirect) {
            Some(self.frame_slot(Layout { size: 8, align: 8 }, function.site)?)
        } else {
            None
        };
        self.flow_labels.clear();
        self.vla_targets.clone_from(&function.vla_targets);
        let epilogue = self.label();
        self.text.push('\n');
        if function.linkage == Linkage::External {
            writeln!(self.text, ".global {name}").unwrap();
        }
        writeln!(self.text, ".type {name}, @function\n{name}:").unwrap();
        self.text
            .push_str("    stp x29, x30, [sp, #-16]!\n    mov x29, sp\n");
        let stack_adjustment = self.text.len();
        self.variadic = if *variadic {
            Some(self.save_variadic_arguments(slots, function.site)?)
        } else {
            None
        };
        if let Some(distance) = indirect_result {
            self.offset_address("x9", "x29", -(distance as i64));
            self.text.push_str("    str x8, [x9]\n");
        }

        // A stack argument can precede a register argument from the other
        // bank. Save incoming registers before the stack copies use x0 or v0.
        for on_stack in [false, true] {
            for (index, (&(object, location), param)) in
                parameters.iter().zip(&function.params).enumerate()
            {
                if matches!(location, ArgumentLocation::Stack(_)) == on_stack {
                    self.receive_parameter(index, &param.ty, &abi_params[index], object, location);
                }
            }
        }
        self.return_context = Some((function, epilogue.clone()));
        self.statements(body, result, &epilogue)?;
        self.return_context = None;
        // Record-valued calls in the body reserve fixed frame slots.
        let tail = self.text.len();
        let frame = self.frame.next_multiple_of(STACK_ALIGNMENT);
        self.adjust_stack("sub", frame);
        let realign = self.frame_alignment > STACK_ALIGNMENT;
        if realign {
            // AAPCS64 §§6.1.1, 6.4.5–6.4.6. Keep x29 at the frame
            // record and use a preserved base for the over-aligned area.
            let mask = !((self.frame_alignment - 1) as u64);
            writeln!(
                self.text,
                "    str x19, [sp, #-16]!\n    mov x19, sp\n    and x19, x19, #{mask:#x}"
            )
            .unwrap();
            self.add_sub_offset("sub", "sp", "x19", self.aligned_frame as u64);
        }
        if let Some(distance) = self.statement_base {
            // Record the bottom of the fixed frame after any x19 realignment.
            self.offset_address("x9", "x29", -(distance as i64));
            self.text.push_str("    mov x10, sp\n    str x10, [x9]\n");
        }
        let adjustment = self.text.split_off(tail);
        self.text.insert_str(stack_adjustment, &adjustment);
        if function.linkage == Linkage::External
            && name == "main"
            && result.kind == Kind::Integer(Integer::Int)
        {
            self.text.push_str("    mov w0, #0\n");
        }
        if let Some(object) = object_result {
            let leave = self.label();
            writeln!(self.text, "    b {leave}\n{epilogue}:").unwrap();
            self.return_object(object, indirect_result);
            writeln!(self.text, "{leave}:").unwrap();
        } else {
            writeln!(self.text, "{epilogue}:").unwrap();
        }
        if realign {
            self.offset_address("x9", "x29", -((frame + STACK_ALIGNMENT) as i64));
            self.text.push_str("    ldr x19, [x9]\n");
        }
        writeln!(
            self.text,
            "    mov sp, x29\n    ldp x29, x30, [sp], #16\n    ret\n.size {name}, . - {name}"
        )
        .unwrap();
        Ok(())
    }

    // Floating values use s0/d0/q0. x0 holds addresses, aggregate/vector
    // representations in memory, and integers extended to their C type.
    fn value(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        match expr.ty.kind {
            Kind::Void | Kind::Array(..) | Kind::Function { .. } => {}
            _ => require_value(&expr.ty, expr.position())?,
        }
        match &expr.kind {
            ExpressionKind::Integer(bits) => {
                constant(&mut self.text, "x0", *bits);
                self.normalize(&expr.ty);
            }
            ExpressionKind::Float(bits) => {
                if expr.ty.kind == Kind::LongDouble {
                    self.binary128_constant(*bits);
                } else {
                    self.float_constant(&expr.ty, *bits as u64);
                }
            }
            ExpressionKind::String(_)
            | ExpressionKind::Identifier(_)
            | ExpressionKind::Local(_)
            | ExpressionKind::ThreadLocal(_)
            | ExpressionKind::CompoundLiteral { .. }
            | ExpressionKind::StaticLiteral(_)
            | ExpressionKind::Member { .. } => {
                if expr.ty.qualifiers.atomic {
                    self.atomic_plain_load(expr)?;
                } else {
                    self.address(expr)?;
                    self.load_value(&expr.ty, expr.bit_field(), "x0");
                }
            }
            ExpressionKind::Unary(Unary::Dereference, _) if expr.ty.qualifiers.atomic => {
                self.atomic_plain_load(expr)?;
            }
            ExpressionKind::Unary(op, operand) => self.unary(*op, operand, &expr.ty)?,
            ExpressionKind::Binary {
                op,
                left,
                right,
                left_type,
                right_type,
            } => {
                self.binary(*op, (left, left_type), (right, right_type), &expr.ty)?;
            }
            ExpressionKind::Cast(operand) => self.converted(operand, &expr.ty)?,
            ExpressionKind::StatementExpression { region, statements, value } => {
                self.statement_expression(*region, statements, value.as_deref(), expr)?;
            }
            ExpressionKind::Assignment {
                left,
                right,
                operation,
            } => self.assignment(left, right, operation.as_ref())?,
            ExpressionKind::Update {
                operand,
                increment,
                postfix,
            } => self.update(operand, *increment, *postfix)?,
            ExpressionKind::Conditional { condition, yes, no } => {
                self.conditional(condition, yes, no, &expr.ty)?;
            }
            ExpressionKind::OmittedConditional {
                value,
                fallback,
                saved,
            } => {
                self.assignment(saved, value, None)?;
                self.conditional(saved, saved, fallback, &expr.ty)?;
            }
            ExpressionKind::Comma(left, right) => {
                self.value(left)?;
                self.value(right)?;
            }
            ExpressionKind::Unreachable => writeln!(self.text, "    .long 0xd4200000").unwrap(),
            ExpressionKind::FltRounds => {
                // DDI 0487 M.b C5.2.8 FPCR.RMode [23:22] is nearest, +infinity,
                // -infinity, toward 0; N1570 5.2.4.2.2 maps these to 1, 2, 3, 0.
                self.text.push_str("    mrs x0, fpcr\n    lsr x0, x0, #22\n    and x0, x0, #3\n    add x0, x0, x0\n    mov x1, #57\n    lsr x0, x1, x0\n    and x0, x0, #3\n");
            }
            ExpressionKind::Atomic { op, weak, arguments } => {
                self.atomic_builtin(*op, *weak, arguments)?;
            }
            ExpressionKind::FloatClass { operand, values } => self.float_class(operand, *values)?,
            ExpressionKind::SignBit(operand) => self.float_sign(operand)?,
            ExpressionKind::Alloca(size) => self.alloca(size)?,
            ExpressionKind::VaStart(list) => self.va_start(list)?,
            ExpressionKind::VaArg(list) => self.va_arg(list, &expr.ty, expr.position())?,
            ExpressionKind::VaCopy { dest, source } => self.va_copy(dest, source)?,
            // There is no runtime cleanup, but evaluating the operand can have side effects.
            ExpressionKind::VaEnd(list) => self.value(list)?,
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments, &expr.ty)?;
                self.normalize(&expr.ty);
            }
        }
        Ok(())
    }

    fn conditional(
        &mut self,
        condition: &Expression<'src>,
        yes: &Expression<'src>,
        no: &Expression<'src>,
        ty: &Type,
    ) -> Result<(), Diagnostic> {
        let otherwise = self.label();
        let end = self.label();
        self.jump_if(condition, false, &otherwise)?;
        self.converted(yes, ty)?;
        writeln!(self.text, "    b {end}\n{otherwise}:").unwrap();
        self.converted(no, ty)?;
        writeln!(self.text, "{end}:").unwrap();
        Ok(())
    }

    fn converted(&mut self, expr: &Expression<'src>, dest: &Type) -> Result<(), Diagnostic> {
        if dest.kind != Kind::Void {
            require_value(dest, expr.position())?;
        }
        self.value(expr)?;
        self.convert(&expr.ty, dest);
        Ok(())
    }

    fn unary(
        &mut self,
        op: Unary,
        operand: &Expression<'src>,
        result: &Type,
    ) -> Result<(), Diagnostic> {
        match op {
            Unary::Dereference => {
                self.value(operand)?;
                self.load(result, "x0");
            }
            Unary::Address => self.address(operand)?,
            Unary::Not => {
                self.value(operand)?;
                self.compare_zero(&operand.ty);
                self.text.push_str("    cset w0, eq\n");
            }
            Unary::Plus | Unary::Minus | Unary::Complement => {
                self.converted(operand, result)?;
                match op {
                    Unary::Minus => match result.kind {
                        Kind::LongDouble => self.negate_binary128(),
                        Kind::Float(precision) => {
                            let fp = float::prefix(precision);
                            writeln!(self.text, "    fneg {fp}0, {fp}0").unwrap();
                        }
                        _ => self.text.push_str("    sub x0, xzr, x0\n"),
                    },
                    Unary::Complement => self.text.push_str("    mvn x0, x0\n"),
                    _ => return Ok(()),
                }
                self.normalize(result);
            }
        }
        Ok(())
    }

    fn binary(
        &mut self,
        op: Binary,
        (left, left_ty): (&Expression<'src>, &Type),
        (right, right_ty): (&Expression<'src>, &Type),
        result: &Type,
    ) -> Result<(), Diagnostic> {
        if matches!(op, Binary::LogicalAnd | Binary::LogicalOr) {
            let end = self.label();
            let branch = if op == Binary::LogicalAnd {
                "cbz"
            } else {
                "cbnz"
            };
            self.value(left)?;
            self.boolean(&left.ty);
            writeln!(self.text, "    {branch} w0, {end}").unwrap();
            self.value(right)?;
            self.boolean(&right.ty);
            writeln!(self.text, "{end}:").unwrap();
        } else {
            self.converted(left, left_ty)?;
            self.push_value(value_prefix(left_ty));
            self.converted(right, right_ty)?;
            self.combine(op, left_ty, right_ty, result);
        }
        Ok(())
    }

    // Consume the spilled left operand and combine it with the right operand.
    fn combine(&mut self, op: Binary, left: &Type, right: &Type, result: &Type) {
        if let Kind::Float(precision) = left.kind {
            self.float_binary(op, precision);
            return;
        }
        if left.kind == Kind::LongDouble {
            self.binary128_binary(op);
            return;
        }
        self.text
            .push_str("    mov x9, x0\n    ldr x0, [sp], #16\n");
        match (&left.kind, &right.kind, op) {
            (Kind::Pointer(element), Kind::Integer(_), Binary::Add | Binary::Subtract) => {
                self.scale_index("x9", element);
            }
            (Kind::Integer(_), Kind::Pointer(element), Binary::Add) => {
                self.scale_index("x0", element);
            }
            (Kind::Pointer(element), Kind::Pointer(_), Binary::Subtract) => {
                self.text.push_str("    sub x0, x0, x9\n");
                let size = element.layout().unwrap().size;
                if size != 1 {
                    constant(&mut self.text, "x9", size as u64);
                    self.text.push_str("    sdiv x0, x0, x9\n");
                }
                return;
            }
            _ => {}
        }
        let signed = matches!(left.kind, Kind::Integer(i) if i.is_signed(Target::Aarch64));
        self.integer_binary(op, signed);
        self.normalize(result);
    }

    fn scale_index(&mut self, reg: &str, element: &Type) {
        let size = element.layout().unwrap().size;
        if size != 1 {
            constant(&mut self.text, "x10", size as u64);
            writeln!(self.text, "    mul {reg}, {reg}, x10").unwrap();
        }
    }

    fn integer_binary(&mut self, op: Binary, signed: bool) {
        use Binary::*;
        let instruction = match op {
            Add => "add",
            Subtract => "sub",
            Multiply => "mul",
            BitAnd => "and",
            BitOr => "orr",
            BitXor => "eor",
            ShiftLeft => "lslv",
            ShiftRight if signed => "asrv",
            ShiftRight => "lsrv",
            Divide if signed => "sdiv",
            Divide => "udiv",
            Remainder => {
                let divide = if signed { "sdiv" } else { "udiv" };
                writeln!(
                    self.text,
                    "    {divide} x10, x0, x9\n    msub x0, x10, x9, x0"
                )
                .unwrap();
                return;
            }
            _ => {
                let condition = match (op, signed) {
                    (Equal, _) => "eq",
                    (NotEqual, _) => "ne",
                    (Less, true) => "lt",
                    (Less, false) => "lo",
                    (LessEqual, true) => "le",
                    (LessEqual, false) => "ls",
                    (Greater, true) => "gt",
                    (Greater, false) => "hi",
                    (GreaterEqual, true) => "ge",
                    (GreaterEqual, false) => "hs",
                    _ => unreachable!(),
                };
                writeln!(self.text, "    cmp x0, x9\n    cset w0, {condition}").unwrap();
                return;
            }
        };
        writeln!(self.text, "    {instruction} x0, x0, x9").unwrap();
    }

    fn push_value(&mut self, register: char) {
        writeln!(self.text, "    str {register}0, [sp, #-16]!").unwrap();
    }

    fn adjust_stack(&mut self, instruction: &str, bytes: usize) {
        if bytes != 0 {
            self.add_sub_offset(instruction, "sp", "sp", bytes as u64);
        }
    }

    fn offset_address(&mut self, dest: &str, base: &str, offset: i64) {
        let op = if offset < 0 { "sub" } else { "add" };
        self.add_sub_offset(op, dest, base, offset.unsigned_abs());
    }

    fn add_sub_offset(&mut self, op: &str, dest: &str, base: &str, bytes: u64) {
        if bytes <= 4095 {
            writeln!(self.text, "    {op} {dest}, {base}, #{bytes}").unwrap();
        } else if bytes & 4095 == 0 && bytes >> 12 <= 4095 {
            writeln!(
                self.text,
                "    {op} {dest}, {base}, #{}, lsl #12",
                bytes >> 12
            )
            .unwrap();
        } else {
            constant(&mut self.text, "x12", bytes);
            writeln!(self.text, "    {op} {dest}, {base}, x12").unwrap();
        }
    }

    fn load(&mut self, ty: &Type, address: &str) {
        let (op, width) = match ty.kind {
            Kind::Integer(integer) => {
                match (integer.layout().size, integer.is_signed(Target::Aarch64)) {
                    (1, false) => ("ldrb", 'w'),
                    (2, false) => ("ldrh", 'w'),
                    (4, false) => ("ldr", 'w'),
                    (1, true) => ("ldrsb", 'x'),
                    (2, true) => ("ldrsh", 'x'),
                    (4, true) => ("ldrsw", 'x'),
                    _ => ("ldr", 'x'),
                }
            }
            Kind::Float(precision) => ("ldr", float::prefix(precision)),
            Kind::LongDouble => ("ldr", 'q'),
            Kind::Pointer(_) => ("ldr", 'x'),
            Kind::Array(..) | Kind::Function { .. } => return,
            Kind::Record(_) | Kind::ShortVector { .. } => {
                if address != "x0" {
                    writeln!(self.text, "    mov x0, {address}").unwrap();
                }
                return;
            }
            _ => unreachable!(),
        };
        writeln!(self.text, "    {op} {width}0, [{address}]").unwrap();
    }

    fn store_object(&mut self, ty: &Type, register: usize, address: &str) {
        let (op, width) = match ty.kind {
            Kind::Float(precision) => ("str", float::prefix(precision)),
            Kind::LongDouble => ("str", 'q'),
            _ => match ty.layout().unwrap().size {
                1 => ("strb", 'w'),
                2 => ("strh", 'w'),
                4 => ("str", 'w'),
                8 => ("str", 'x'),
                _ => unreachable!(),
            },
        };
        writeln!(self.text, "    {op} {width}{register}, [{address}]").unwrap();
    }

    fn normalize(&mut self, ty: &Type) {
        let Kind::Integer(integer) = ty.kind else {
            return;
        };
        let instruction = match (integer.layout().size, integer.is_signed(Target::Aarch64)) {
            (1, true) => "sxtb x0, w0",
            (1, false) => "uxtb w0, w0",
            (2, true) => "sxth x0, w0",
            (2, false) => "uxth w0, w0",
            (4, true) => "sxtw x0, w0",
            (4, false) => "mov w0, w0",
            _ => return,
        };
        writeln!(self.text, "    {instruction}").unwrap();
    }

    fn convert(&mut self, source: &Type, dest: &Type) {
        if source.kind == dest.kind {
            return;
        }
        match (&source.kind, &dest.kind) {
            (_, Kind::Integer(Integer::Bool)) => self.boolean(source),
            (scalar @ (Kind::Integer(_) | Kind::Float(_)), Kind::LongDouble) => {
                self.widen_binary128(scalar);
            }
            (Kind::LongDouble, scalar @ (Kind::Integer(_) | Kind::Float(_))) => {
                self.narrow_binary128(scalar);
                self.normalize(dest);
            }
            (Kind::Integer(integer), Kind::Float(precision)) => {
                let op = if integer.is_signed(Target::Aarch64) {
                    "scvtf"
                } else {
                    "ucvtf"
                };
                let fp = float::prefix(*precision);
                writeln!(self.text, "    {op} {fp}0, x0").unwrap();
            }
            (Kind::Float(precision), Kind::Integer(integer)) => {
                let op = if integer.is_signed(Target::Aarch64) {
                    "fcvtzs"
                } else {
                    "fcvtzu"
                };
                let fp = float::prefix(*precision);
                writeln!(self.text, "    {op} x0, {fp}0").unwrap();
                self.normalize(dest);
            }
            (Kind::Float(from), Kind::Float(to)) => {
                let from = float::prefix(*from);
                let to = float::prefix(*to);
                writeln!(self.text, "    fcvt {to}0, {from}0").unwrap();
            }
            _ => self.normalize(dest),
        }
    }

    fn compare_zero(&mut self, ty: &Type) {
        match ty.kind {
            Kind::LongDouble => self.compare_binary128_zero(),
            Kind::Float(precision) => {
                let fp = float::prefix(precision);
                writeln!(self.text, "    fcmp {fp}0, #0.0").unwrap();
            }
            _ => self.text.push_str("    cmp x0, #0\n"),
        }
    }

    fn boolean(&mut self, ty: &Type) {
        self.compare_zero(ty);
        self.text.push_str("    cset w0, ne\n");
    }

    fn label(&mut self) -> String {
        let label = format!(".L.bcc.{}", self.next_label);
        self.next_label += 1;
        label
    }

    fn use_symbol(&mut self, name: &'src str, site: &Position) -> Result<(), Diagnostic> {
        check_symbol(name, site)?;
        if self.object_definitions.contains(name) {
            return Ok(());
        }
        match self.functions.get(name) {
            Some(function) if function.body.is_some() => {}
            Some(function) if function.linkage == Linkage::Internal => {
                return Err(Diagnostic::new(
                    site,
                    "used static function has no definition",
                ));
            }
            _ => {
                self.externs.insert(name);
            }
        }
        Ok(())
    }
}

fn check_symbol(name: &str, site: &Position) -> Result<(), Diagnostic> {
    if name.contains('\\') {
        return Err(Diagnostic::new(
            site,
            "code generation for universal characters in symbol names is not implemented",
        ));
    }
    Ok(())
}

fn require_value(ty: &Type, site: &Position) -> Result<(), Diagnostic> {
    if matches!(
        ty.kind,
        Kind::Integer(_)
            | Kind::Pointer(_)
            | Kind::Float(_)
            | Kind::LongDouble
            | Kind::ShortVector { .. }
    ) || matches!(ty.kind, Kind::Record(record) if record.layout.is_some())
    {
        Ok(())
    } else {
        Err(Diagnostic::new(
            site,
            "value is outside the supported AArch64 subset",
        ))
    }
}

fn value_prefix(ty: &Type) -> char {
    match ty.kind {
        Kind::Float(precision) => float::prefix(precision),
        Kind::LongDouble => 'q',
        _ => 'x',
    }
}

fn constant(out: &mut String, reg: &str, bits: u64) {
    writeln!(out, "    movz {reg}, #{}", bits & 0xffff).unwrap();
    for shift in [16, 32, 48] {
        let part = (bits >> shift) & 0xffff;
        if part != 0 {
            writeln!(out, "    movk {reg}, #{part}, lsl #{shift}").unwrap();
        }
    }
}
