//! Unoptimized AArch64 scalar assembly for bas.
//!
//! AAPCS64 2025Q1 §§6.1, 6.4.5–6.4.6, 6.8–6.9 and §10.1.1 Table 3
//! define the register, stack and C type conventions. A64 instructions
//! and their operand forms follow Arm DDI 0487 M.b Chapters C6–C7.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use crate::pp::{Diagnostic, Position, Target};
use crate::sema::{
    Argument, Binary, Expression, ExpressionKind, Function, Linkage, LocalStorage, Statement, TranslationUnit,
    Unary,
};
use crate::types::{Float, Integer, Kind, Type};

mod abi;
mod float;
mod flow;

use abi::{ArgumentLocation, ArgumentSlots};
use flow::Slot;

const ARGUMENT_REGISTERS: usize = 8;
const FRAME_RECORD: usize = 16;

pub(super) fn emit(unit: &TranslationUnit<'_>) -> Result<String, Diagnostic> {
    let mut functions = HashMap::new();
    for function in &unit.functions {
        let entry = functions.entry(function.name).or_insert(function);
        if function.body.is_some() {
            *entry = function;
        }
    }
    let mut emitter = Emitter {
        functions,
        locals: Vec::new(),
        flow_labels: HashMap::new(),
        externs: BTreeSet::new(),
        text: String::from(".section .text\n"),
        rodata: String::new(),
        bss: String::new(),
        next_label: 0,
    };
    for function in &unit.functions {
        if let Some(body) = &function.body {
            emitter.function(function, body)?;
        }
    }
    for name in emitter.externs {
        writeln!(emitter.text, ".extern {name}").unwrap();
    }
    if !emitter.rodata.is_empty() {
        emitter.text.push_str("\n.section .rodata\n");
        emitter.text.push_str(&emitter.rodata);
    }
    if !emitter.bss.is_empty() {
        emitter.text.push_str("\n.section .bss\n");
        emitter.text.push_str(&emitter.bss);
    }
    Ok(emitter.text)
}

struct Emitter<'unit, 'src> {
    functions: HashMap<&'src str, &'unit Function<'src>>,
    locals: Vec<Slot>,
    flow_labels: HashMap<usize, String>,
    externs: BTreeSet<&'src str>,
    text: String,
    rodata: String,
    bss: String,
    next_label: usize,
}

impl<'src> Emitter<'_, 'src> {
    fn function(
        &mut self,
        function: &Function<'src>,
        body: &[Statement<'src>],
    ) -> Result<(), Diagnostic> {
        let name = function.name;
        check_symbol(name, function.site)?;
        let Kind::Function { result, .. } = &function.ty.kind else {
            unreachable!()
        };
        if result.kind != Kind::Void {
            require_scalar(result, function.site)?;
        }
        for param in &function.params {
            require_scalar(&param.ty, param.name.unwrap().1)?;
        }
        let frame = self.layout(function)?;
        self.flow_labels.clear();
        let epilogue = self.label();
        self.text.push('\n');
        if function.linkage == Linkage::External {
            writeln!(self.text, ".global {name}").unwrap();
        }
        writeln!(self.text, ".type {name}, @function\n{name}:").unwrap();
        self.text
            .push_str("    stp x29, x30, [sp, #-16]!\n    mov x29, sp\n");
        self.adjust_stack("sub", frame);

        let mut slots = ArgumentSlots::default();
        for (index, param) in function.params.iter().enumerate() {
            self.local_address(index, 0, "x10");
            let register = match slots.next(&param.ty) {
                ArgumentLocation::Register { index, .. } => index,
                ArgumentLocation::Stack(offset) => {
                    self.offset_address("x9", "x29", (FRAME_RECORD + offset) as i64);
                    self.load(&param.ty, "x9");
                    0
                }
            };
            self.store_object(&param.ty, register, "x10");
        }
        self.statements(body, result, &epilogue)?;
        if function.linkage == Linkage::External
            && name == "main"
            && result.kind == Kind::Integer(Integer::Int)
        {
            self.text.push_str("    mov w0, #0\n");
        }
        writeln!(
            self.text,
            "{epilogue}:\n    mov sp, x29\n    ldp x29, x30, [sp], #16\n    ret\n.size {name}, . - {name}"
        )
        .unwrap();
        Ok(())
    }

    // Values use s0/d0 for float/double and x0 for addresses and integers
    // extended according to their C type. Spill across recursive evaluation.
    fn value(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        match expr.ty.kind {
            Kind::Void | Kind::Array(..) | Kind::Function { .. } => {}
            _ => require_scalar(&expr.ty, expr.position())?,
        }
        match &expr.kind {
            ExpressionKind::Integer(bits) => {
                constant(&mut self.text, "x0", *bits);
                self.normalize(&expr.ty);
            }
            ExpressionKind::Float(bits) => self.float_constant(&expr.ty, *bits as u64),
            ExpressionKind::String(_)
            | ExpressionKind::Identifier(_)
            | ExpressionKind::Local(_) => {
                self.address(expr)?;
                self.load(&expr.ty, "x0");
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
                let otherwise = self.label();
                let end = self.label();
                self.jump_if(condition, false, &otherwise)?;
                self.converted(yes, &expr.ty)?;
                writeln!(self.text, "    b {end}\n{otherwise}:").unwrap();
                self.converted(no, &expr.ty)?;
                writeln!(self.text, "{end}:").unwrap();
            }
            ExpressionKind::ThreadLocal(_)
            | ExpressionKind::Member { .. }
            | ExpressionKind::VaStart(_)
            | ExpressionKind::VaArg(_)
            | ExpressionKind::VaCopy { .. }
            | ExpressionKind::VaEnd(_) => {
                return Err(Diagnostic::new(
                    expr.position(),
                    "AArch64 aggregate, variadic and C TLS support is deferred to A4",
                ));
            }
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments)?;
                self.normalize(&expr.ty);
            }
        }
        Ok(())
    }

    fn converted(&mut self, expr: &Expression<'src>, dest: &Type) -> Result<(), Diagnostic> {
        if dest.kind != Kind::Void {
            require_scalar(dest, expr.position())?;
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
                    Unary::Minus => {
                        if let Kind::Float(precision) = result.kind {
                            let fp = float::prefix(precision);
                            writeln!(self.text, "    fneg {fp}0, {fp}0").unwrap();
                        } else {
                            self.text.push_str("    sub x0, xzr, x0\n");
                        }
                    }
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
            Kind::Pointer(_) => ("ldr", 'x'),
            Kind::Array(..) | Kind::Function { .. } => return,
            _ => unreachable!(),
        };
        writeln!(self.text, "    {op} {width}0, [{address}]").unwrap();
    }

    fn store_object(&mut self, ty: &Type, register: usize, address: &str) {
        let (op, width) = if let Kind::Float(precision) = ty.kind {
            ("str", float::prefix(precision))
        } else {
            match ty.layout().unwrap().size {
                1 => ("strb", 'w'),
                2 => ("strh", 'w'),
                4 => ("str", 'w'),
                8 => ("str", 'x'),
                _ => unreachable!(),
            }
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
        if let Kind::Float(precision) = ty.kind {
            let fp = float::prefix(precision);
            writeln!(self.text, "    fcmp {fp}0, #0.0").unwrap();
        } else {
            self.text.push_str("    cmp x0, #0\n");
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

fn require_scalar(ty: &Type, site: &Position) -> Result<(), Diagnostic> {
    if matches!(
        ty.kind,
        Kind::Integer(_) | Kind::Pointer(_) | Kind::Float(_)
    ) {
        Ok(())
    } else {
        Err(Diagnostic::new(
            site,
            "value is outside the AArch64 scalar subset",
        ))
    }
}

fn value_prefix(ty: &Type) -> char {
    match ty.kind {
        Kind::Float(precision) => float::prefix(precision),
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
