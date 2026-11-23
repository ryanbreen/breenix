//! Unoptimized SysV AMD64 assembly in bas's Intel-style syntax.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use crate::pp::{Diagnostic, Position, Target};
use crate::sema::{
    Argument, Binary, Expression, ExpressionKind, Function, Linkage, LocalStorage, Object,
    Statement, StaticAddress, ThreadData, ThreadInitializer, TranslationUnit, Unary,
};
use crate::types::{BitField, Float, Integer, Kind, Layout, Record, Type};

mod abi;
mod bit_field;
mod flow;
mod thread;
mod varargs;
mod x87;

use abi::{CallLayout, Place, RecordAbi, RecordRegisters, Register};
use flow::Slot;
use thread::ThreadPlace;
use varargs::VariadicFrame;

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
        records: &unit.records,
        frame: 0,
        locals: Vec::new(),
        variadic: None,
        flow_labels: HashMap::new(),
        data: String::new(),
        tdata: String::new(),
        tbss: String::new(),
        externs: BTreeSet::new(),
        thread_externs: BTreeSet::new(),
        thread_definitions: unit
            .thread_objects
            .iter()
            .map(|thread| thread.object.name)
            .collect(),
        text: String::from("section .text\n"),
        rodata: String::new(),
        string_count: 0,
        next_label: 0,
    };
    for thread in &unit.thread_objects {
        emitter.thread_definition(
            &thread.object,
            thread.object.name,
            thread.linkage,
            thread.initializer.as_ref(),
        )?;
    }
    for function in &unit.functions {
        if let Some(body) = &function.body {
            emitter.function(function, body)?;
        }
    }
    for name in emitter.externs {
        writeln!(emitter.text, "extern {name}").unwrap();
    }
    for name in emitter.thread_externs {
        writeln!(emitter.text, "extern {name}\n.type {name}, @tls").unwrap();
    }
    if !emitter.rodata.is_empty() {
        emitter.text.push_str("\nsection .rodata\n");
        emitter.text.push_str(&emitter.rodata);
    }
    if !emitter.data.is_empty() {
        emitter.text.push_str("\nsection .bss\n");
        emitter.text.push_str(&emitter.data);
    }
    for (section, data) in [(".tdata", emitter.tdata), (".tbss", emitter.tbss)] {
        if !data.is_empty() {
            writeln!(emitter.text, "\nsection {section}").unwrap();
            emitter.text.push_str(&data);
        }
    }
    Ok(emitter.text)
}

struct Emitter<'unit, 'src> {
    functions: HashMap<&'src str, &'unit Function<'src>>,
    records: &'unit [Record<'src>],
    frame: usize,
    locals: Vec<Slot>,
    variadic: Option<VariadicFrame>,
    flow_labels: HashMap<usize, String>,
    data: String,
    tdata: String,
    tbss: String,
    externs: BTreeSet<&'src str>,
    thread_externs: BTreeSet<&'src str>,
    thread_definitions: BTreeSet<&'src str>,
    text: String,
    rodata: String,
    string_count: usize,
    next_label: usize,
}

impl<'src> Emitter<'_, 'src> {
    fn function(
        &mut self,
        function: &Function<'src>,
        body: &[Statement<'src>],
    ) -> Result<(), Diagnostic> {
        let name = function.name;
        self.check_symbol(name, function.site)?;
        let Kind::Function {
            result, variadic, ..
        } = &function.ty.kind
        else {
            unreachable!()
        };
        if result.kind != Kind::Void && !supported_abi_type(result) {
            return Err(Diagnostic::new(
                function.site,
                "function result is outside the supported AMD64 ABI subset",
            ));
        }
        for param in &function.params {
            if !supported_abi_type(&param.ty) {
                return Err(Diagnostic::new(
                    param.name.unwrap().1,
                    "parameter is outside the supported AMD64 ABI subset",
                ));
            }
        }
        self.layout(function)?;
        let record_result = self.record_abi(result);
        let sret = if matches!(record_result, Some(RecordAbi::Memory)) {
            Some(self.frame_slot(Layout { size: 8, align: 8 }, function.site)?)
        } else {
            None
        };
        self.flow_labels.clear();
        let epilogue = self.label();
        self.text.push('\n');
        if function.linkage == Linkage::External {
            writeln!(self.text, "global {name}").unwrap();
        }
        writeln!(self.text, ".type {name}, @function\n{name}:").unwrap();
        // AMD64 psABI §3.2.2: RSP + 8 is aligned to 16 on entry. Saving RBP
        // aligns RSP for calls. Round stack allocations to 16 bytes and
        // keep spills above RSP.
        self.text.push_str("    push rbp\n    mov rbp, rsp\n");
        let stack_adjustment = self.text.len();

        let save = if *variadic {
            Some(self.save_variadic_registers(function.site)?)
        } else {
            None
        };
        let mut layout = CallLayout::default();
        if let Some(slot) = &sret {
            writeln!(self.text, "    mov qword {}, rdi", slot.memory(0)).unwrap();
            layout.integers = 1;
        }
        let mut stack_records = Vec::new();
        for (index, param) in function.params.iter().enumerate() {
            let memory = self.locals[index].memory(0);
            match layout.place(&param.ty, self.records, param.name.unwrap().1)? {
                Place::Integer(reg) => {
                    writeln!(self.text, "    mov qword {memory}, {reg}").unwrap()
                }
                Place::Vector(reg, float) => {
                    let instruction = float_move(float);
                    writeln!(self.text, "    {instruction} {memory}, xmm{reg}").unwrap();
                }
                Place::Record(registers) => {
                    let slot = &self.locals[index];
                    Self::save_record_registers(&mut self.text, registers, |piece| {
                        slot.memory(piece)
                    });
                }
                Place::Stack(offset) if matches!(param.ty.kind, Kind::Record(_)) => {
                    stack_records.push((index, offset, param.ty.layout().unwrap().size));
                }
                Place::Stack(offset) if param.ty.kind == Kind::LongDouble => {
                    let source = offset + 16;
                    writeln!(
                        self.text,
                        "    fld tbyte [rbp + {source}]\n    fstp tbyte {memory}"
                    )
                    .unwrap();
                }
                Place::Stack(offset) => {
                    // The saved RBP and return address precede the input area.
                    let source = offset + 16;
                    writeln!(
                        self.text,
                        "    mov rax, qword [rbp + {source}]\n    mov qword {memory}, rax"
                    )
                    .unwrap();
                }
            }
        }
        self.variadic = save.map(|save| VariadicFrame {
            save,
            named: layout,
        });
        // Spill register inputs before rep movsb reuses argument registers.
        for (index, offset, size) in stack_records {
            let source = offset + 16;
            let dest = self.locals[index].memory(0);
            writeln!(
                self.text,
                "    lea rsi, [rbp + {source}]\n    lea rdi, {dest}"
            )
            .unwrap();
            self.copy_record(size);
        }
        self.statements(body, result, &epilogue)?;
        // Calls in the body allocate result temporaries in the fixed frame.
        let frame = self.frame.next_multiple_of(16);
        if frame != 0 {
            self.text
                .insert_str(stack_adjustment, &format!("    sub rsp, {frame}\n"));
        }
        if name == "main"
            && function.linkage == Linkage::External
            && result.kind == Kind::Integer(Integer::Int)
        {
            // N1570 §5.1.2.2.3 specifies the result when int main falls through.
            self.text.push_str("    mov eax, 0\n");
        }
        if let Some(abi) = record_result {
            // Only explicit returns have a record address in RAX.
            let leave = self.label();
            writeln!(self.text, "    jmp {leave}\n{epilogue}:").unwrap();
            if let Some(slot) = sret {
                writeln!(
                    self.text,
                    "    mov rsi, rax\n    mov rdi, qword {}",
                    slot.memory(0)
                )
                .unwrap();
                self.copy_record(result.layout().unwrap().size);
            } else {
                self.text.push_str("    mov r11, rax\n");
                self.load_record_registers(
                    abi.result_registers().unwrap(),
                    result.layout().unwrap().size,
                    |piece| format!("[r11 + {piece}]"),
                );
            }
            writeln!(self.text, "{leave}:").unwrap();
        } else {
            writeln!(self.text, "{epilogue}:").unwrap();
        }
        writeln!(self.text, "    leave\n    ret\n.size {name}, . - {name}").unwrap();
        Ok(())
    }

    // Canonical value locations: a normalized integer or pointer in RAX,
    // float/double in XMM0, a record's address in RAX, or one x87 stack
    // entry in ST(0). Spill or consume that entry before evaluating another
    // expression so no x87 temporaries are live across calls.
    fn value(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        match &expr.kind {
            ExpressionKind::Integer(value) => {
                writeln!(self.text, "    mov rax, 0x{value:x}").unwrap();
                self.normalize_integer(&expr.ty);
            }
            ExpressionKind::Float(bits) => match expr.ty.kind {
                Kind::Float(float) => self.float_constant(*bits as u64, float, 0),
                Kind::LongDouble => self.long_double_constant(*bits),
                _ => unreachable!(),
            },
            ExpressionKind::String(_)
            | ExpressionKind::Identifier(_)
            | ExpressionKind::ThreadLocal(_)
            | ExpressionKind::Local(_)
            | ExpressionKind::Member { .. } => {
                let place = self.thread_scalar_place(expr)?;
                if place.is_none() {
                    self.address(expr)?;
                }
                self.load_lvalue(expr, place.as_ref());
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
            } => {
                self.assignment(left, right, operation.as_ref())?;
            }
            ExpressionKind::Update {
                operand,
                increment,
                postfix,
            } => {
                self.update(operand, *increment, *postfix)?;
            }
            ExpressionKind::Conditional { condition, yes, no } => {
                let otherwise = self.label();
                let end = self.label();
                self.jump_if(condition, "jz", &otherwise)?;
                self.converted(yes, &expr.ty)?;
                writeln!(self.text, "    jmp {end}\n{otherwise}:").unwrap();
                self.converted(no, &expr.ty)?;
                writeln!(self.text, "{end}:").unwrap();
            }
            ExpressionKind::VaStart(list) => self.va_start(list)?,
            ExpressionKind::VaArg(list) => self.va_arg(list, &expr.ty, expr.position())?,
            ExpressionKind::VaCopy { dest, source } => self.va_copy(dest, source)?,
            // AMD64 va_end has no resources to release. Evaluate the operand.
            ExpressionKind::VaEnd(list) => self.value(list)?,
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments, &expr.ty)?;
                self.normalize_integer(&expr.ty);
            }
        }
        Ok(())
    }

    fn discard(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        self.converted(expr, &Type::new(Kind::Void))
    }

    fn converted(&mut self, expr: &Expression<'src>, dest: &Type) -> Result<(), Diagnostic> {
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
                self.load(result, "[rax]");
            }
            Unary::Address => self.address(operand)?,
            Unary::Not => {
                self.condition(operand)?;
                self.text.push_str("    xor eax, 1\n");
            }
            Unary::Plus | Unary::Minus | Unary::Complement => {
                self.converted(operand, result)?;
                match (op, &result.kind) {
                    (Unary::Minus, Kind::Float(float)) => {
                        let sign = 1 << (float.layout().size * 8 - 1);
                        self.float_constant(sign, *float, 1);
                        let xor = if *float == Float::Single {
                            "xorps"
                        } else {
                            "xorpd"
                        };
                        writeln!(self.text, "    {xor} xmm0, xmm1").unwrap();
                    }
                    (Unary::Minus, Kind::LongDouble) => self.text.push_str("    fchs\n"),
                    (Unary::Minus, _) => self.text.push_str("    neg rax\n"),
                    (Unary::Complement, _) => self.text.push_str("    not rax\n"),
                    _ => {}
                }
                self.normalize_integer(result);
            }
        }
        Ok(())
    }

    fn condition(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        self.value(expr)?;
        self.boolean(&expr.ty);
        Ok(())
    }

    fn boolean(&mut self, source: &Type) {
        if let Kind::Float(float) = source.kind {
            self.text.push_str("    pxor xmm1, xmm1\n");
            writeln!(self.text, "    ucomi{} xmm0, xmm1", suffix(float)).unwrap();
            // NaN compares unordered, so PF also makes the Boolean result true.
            self.text
                .push_str("    setne al\n    setp dl\n    or al, dl\n");
        } else if source.kind == Kind::LongDouble {
            self.x87_boolean();
        } else {
            self.text.push_str("    test rax, rax\n    setne al\n");
        }
        self.text.push_str("    movzx eax, al\n");
    }

    fn binary(
        &mut self,
        op: Binary,
        (left, left_ty): (&Expression<'src>, &Type),
        (right, right_ty): (&Expression<'src>, &Type),
        result: &Type,
    ) -> Result<(), Diagnostic> {
        if matches!(op, Binary::LogicalAnd | Binary::LogicalOr) {
            self.condition(left)?;
            let end = self.label();
            let skip = if op == Binary::LogicalAnd {
                "jz"
            } else {
                "jnz"
            };
            writeln!(self.text, "    test eax, eax\n    {skip} {end}").unwrap();
            self.condition(right)?;
            writeln!(self.text, "{end}:").unwrap();
            return Ok(());
        }

        self.converted(left, left_ty)?;
        self.adjust_stack("sub", 16);
        self.store(left_ty, "[rsp]");
        self.converted(right, right_ty)?;
        self.combine(op, left_ty, right_ty, result);
        Ok(())
    }

    // The left operand is saved in an aligned slot above the right operand's
    // expression stack. Consume that slot and leave the result in its
    // canonical location.
    fn combine(&mut self, op: Binary, left: &Type, right: &Type, result: &Type) {
        if left.kind == Kind::LongDouble {
            self.load(left, "[rsp]");
            self.adjust_stack("add", 16);
            self.x87_binary(op);
            return;
        }
        if let Kind::Float(float) = left.kind {
            let mov = float_move(float);
            writeln!(self.text, "    {mov} xmm1, xmm0\n    {mov} xmm0, [rsp]").unwrap();
            self.adjust_stack("add", 16);
            self.float_binary(op, float);
            return;
        }
        self.text
            .push_str("    mov rcx, rax\n    mov rax, qword [rsp]\n");
        self.adjust_stack("add", 16);
        match (&left.kind, &right.kind, op) {
            (Kind::Pointer(element), Kind::Integer(_), Binary::Add | Binary::Subtract) => {
                self.scale_pointer_index("rcx", element);
            }
            (Kind::Integer(_), Kind::Pointer(element), Binary::Add) => {
                self.scale_pointer_index("rax", element);
            }
            (Kind::Pointer(element), Kind::Pointer(_), Binary::Subtract) => {
                self.text.push_str("    sub rax, rcx\n");
                let size = element.layout().unwrap().size;
                if size != 1 {
                    writeln!(self.text, "    mov rcx, {size}\n    cqo\n    idiv rcx").unwrap();
                }
                return;
            }
            _ => {}
        }
        let signed =
            matches!(left.kind, Kind::Integer(integer) if integer.is_signed(Target::X86_64));
        self.integer_binary(op, signed);
        self.normalize_integer(result);
    }

    fn scale_pointer_index(&mut self, reg: &str, element: &Type) {
        let size = element.layout().unwrap().size;
        if size != 1 {
            writeln!(self.text, "    mov r10, {size}\n    imul {reg}, r10").unwrap();
        }
    }

    fn integer_binary(&mut self, op: Binary, signed: bool) {
        use Binary::*;
        let instruction = match op {
            Add => "add rax, rcx",
            Subtract => "sub rax, rcx",
            Multiply => "imul rax, rcx",
            BitAnd => "and rax, rcx",
            BitOr => "or rax, rcx",
            BitXor => "xor rax, rcx",
            ShiftLeft => "shl rax, cl",
            ShiftRight if signed => "sar rax, cl",
            ShiftRight => "shr rax, cl",
            Divide | Remainder => {
                if signed {
                    self.text.push_str("    cqo\n    idiv rcx\n");
                } else {
                    self.text.push_str("    xor edx, edx\n    div rcx\n");
                }
                if op == Remainder {
                    self.text.push_str("    mov rax, rdx\n");
                }
                return;
            }
            _ => {
                self.text.push_str("    cmp rax, rcx\n");
                self.compare_result(op, signed, false);
                return;
            }
        };
        writeln!(self.text, "    {instruction}").unwrap();
    }

    fn float_binary(&mut self, op: Binary, float: Float) {
        let suffix = suffix(float);
        let instruction = match op {
            Binary::Add => "add",
            Binary::Subtract => "sub",
            Binary::Multiply => "mul",
            Binary::Divide => "div",
            _ => {
                writeln!(self.text, "    ucomi{suffix} xmm0, xmm1").unwrap();
                self.compare_result(op, false, true);
                return;
            }
        };
        writeln!(self.text, "    {instruction}{suffix} xmm0, xmm1").unwrap();
    }

    fn compare_result(&mut self, op: Binary, signed: bool, float: bool) {
        use Binary::*;
        let cc = match (op, signed) {
            (Equal, _) => "e",
            (NotEqual, _) => "ne",
            (Less, true) => "l",
            (Less, false) => "b",
            (LessEqual, true) => "le",
            (LessEqual, false) => "be",
            (Greater, true) => "g",
            (Greater, false) => "a",
            (GreaterEqual, true) => "ge",
            (GreaterEqual, false) => "ae",
            _ => unreachable!(),
        };
        writeln!(self.text, "    set{cc} al").unwrap();
        if float {
            match op {
                Equal | Less | LessEqual => self.text.push_str("    setnp dl\n    and al, dl\n"),
                NotEqual => self.text.push_str("    setp dl\n    or al, dl\n"),
                _ => {}
            }
        }
        self.text.push_str("    movzx eax, al\n");
    }

    fn call(
        &mut self,
        callee: &Expression<'src>,
        arguments: &[Argument<'src>],
        result: &Type,
    ) -> Result<(), Diagnostic> {
        let (function, direct) = match &callee.ty.kind {
            Kind::Function { .. } => {
                let direct = match callee.kind {
                    ExpressionKind::Identifier(name) => Some(name),
                    _ => None,
                };
                (&callee.ty, direct)
            }
            Kind::Pointer(function) => (function.as_ref(), None),
            _ => unreachable!(),
        };
        let Kind::Function { variadic, .. } = &function.kind else {
            unreachable!()
        };
        if let Some(name) = direct {
            self.use_symbol(name, callee.position())?;
        }
        let site = callee.position();
        let record_result = self.record_abi(result);
        let result_slot = if record_result.is_some() {
            let mut layout = result.layout().unwrap();
            layout.size = abi::eightbyte_size(layout.size, site)?;
            Some(self.frame_slot(layout, site)?)
        } else {
            None
        };
        let sret = matches!(record_result, Some(RecordAbi::Memory));
        let mut layout = CallLayout {
            integers: usize::from(sret),
            ..CallLayout::default()
        };
        let places = arguments
            .iter()
            .map(|arg| layout.place(&arg.ty, self.records, arg.expr.position()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut callee_slot = layout.stack;
        let mut staged = Vec::with_capacity(arguments.len());
        for (arg, place) in arguments.iter().zip(places) {
            let offset = match place {
                Place::Stack(slot) if direct_stack_argument(&arg.ty) => slot,
                _ => {
                    let offset = callee_slot;
                    let bytes = arg.ty.layout().map_or(8, |layout| layout.size);
                    callee_slot =
                        abi::checked_stack_end(offset, abi::eightbyte_size(bytes, site)?, site)?;
                    offset
                }
            };
            staged.push((place, offset));
        }
        let reserve = abi::checked_stack_end(callee_slot, usize::from(direct.is_none()) * 8, site)?
            .next_multiple_of(16);
        self.adjust_stack("sub", reserve);
        if direct.is_none() {
            self.value(callee)?;
            writeln!(self.text, "    mov qword [rsp + {callee_slot}], rax").unwrap();
        }
        // Stage values before loading any registers. Nested calls use the
        // stack below RSP, leaving both the outgoing area and spills intact.
        for (arg, &(_, offset)) in arguments.iter().zip(&staged) {
            self.converted(&arg.expr, &arg.ty)?;
            let dest = format!("[rsp + {offset}]");
            if matches!(arg.ty.kind, Kind::Record(_)) {
                self.store_object(&arg.ty, &dest);
            } else {
                self.store(&arg.ty, &dest);
            }
        }
        if sret {
            let address = result_slot.as_ref().unwrap().memory(0);
            writeln!(self.text, "    lea rdi, {address}").unwrap();
        }
        for (arg, (place, offset)) in arguments.iter().zip(staged) {
            match place {
                Place::Integer(reg) => {
                    writeln!(self.text, "    mov {reg}, qword [rsp + {offset}]").unwrap();
                }
                Place::Vector(reg, float) => {
                    let instruction = float_move(float);
                    writeln!(self.text, "    {instruction} xmm{reg}, [rsp + {offset}]").unwrap();
                }
                Place::Record(registers) => {
                    self.load_record_registers(registers, arg.ty.layout().unwrap().size, |piece| {
                        format!("[rsp + {}]", offset + piece)
                    });
                }
                Place::Stack(_) if direct_stack_argument(&arg.ty) => {}
                Place::Stack(slot) => {
                    let (reg, size) = match arg.ty.kind {
                        Kind::Float(Float::Single) => ("eax", "dword"),
                        _ => ("rax", "qword"),
                    };
                    writeln!(self.text, "    mov {reg}, {size} [rsp + {offset}]\n    mov {size} [rsp + {slot}], {reg}").unwrap();
                }
            }
        }
        if *variadic {
            // SysV AMD64 psABI §3.2.3: AL gives an upper bound on the vector
            // registers used in this call, including the named arguments.
            writeln!(self.text, "    mov eax, {}", layout.vectors).unwrap();
        }
        if let Some(name) = direct {
            writeln!(self.text, "    call {name}@plt").unwrap();
        } else {
            writeln!(self.text, "    call qword [rsp + {callee_slot}]").unwrap();
        }
        self.adjust_stack("add", reserve);
        if let Some((registers, slot)) = record_result
            .and_then(RecordAbi::result_registers)
            .zip(result_slot)
        {
            Self::save_record_registers(&mut self.text, registers, |piece| slot.memory(piece));
            writeln!(self.text, "    lea rax, {}", slot.memory(0)).unwrap();
        }
        // A MEMORY callee already leaves its return buffer address in RAX.
        Ok(())
    }

    fn record_abi(&self, ty: &Type) -> Option<RecordAbi> {
        matches!(ty.kind, Kind::Record(_)).then(|| RecordAbi::classify(ty, self.records))
    }

    // The receiving frame slots are rounded up to full eightbytes.
    fn save_record_registers(
        text: &mut String,
        registers: RecordRegisters,
        address: impl Fn(usize) -> String,
    ) {
        for (index, reg) in registers.into_iter().enumerate() {
            let Some(reg) = reg else { continue };
            let dest = address(index * 8);
            match reg {
                Register::Integer(reg) => writeln!(text, "    mov qword {dest}, {reg}").unwrap(),
                Register::Sse(reg) => writeln!(text, "    movsd {dest}, xmm{reg}").unwrap(),
                Register::X87 => writeln!(text, "    fstp tbyte {dest}").unwrap(),
            }
        }
    }

    fn load_record_registers(
        &mut self,
        registers: RecordRegisters,
        size: usize,
        address: impl Fn(usize) -> String,
    ) {
        for (index, reg) in registers.into_iter().enumerate() {
            let Some(reg) = reg else { continue };
            let offset = index * 8;
            let bytes = (size - offset).min(8);
            let source = address(offset);
            match reg {
                Register::Integer(reg) if bytes == 8 => {
                    writeln!(self.text, "    mov {reg}, qword {source}").unwrap();
                }
                Register::Integer(reg) => {
                    self.load_integer_tail(reg, bytes, |tail| address(offset + tail))
                }
                Register::Sse(reg) => {
                    let instruction = if bytes == 4 { "movss" } else { "movsd" };
                    writeln!(self.text, "    {instruction} xmm{reg}, {source}").unwrap();
                }
                Register::X87 => writeln!(self.text, "    fld tbyte {source}").unwrap(),
            }
        }
    }

    fn load_integer_tail(&mut self, reg: &str, bytes: usize, address: impl Fn(usize) -> String) {
        // Load only the object bytes, even if the source ends at a page boundary.
        writeln!(self.text, "    xor {reg}, {reg}").unwrap();
        let mut consumed = 0;
        for (width, load) in [
            (4, "mov r10d, dword"),
            (2, "movzx r10d, word"),
            (1, "movzx r10d, byte"),
        ] {
            if bytes - consumed >= width {
                writeln!(self.text, "    {load} {}", address(consumed)).unwrap();
                if consumed != 0 {
                    writeln!(self.text, "    shl r10, {}", consumed * 8).unwrap();
                }
                writeln!(self.text, "    or {reg}, r10").unwrap();
                consumed += width;
            }
        }
    }

    fn load(&mut self, ty: &Type, address: &str) {
        let instruction = match ty.kind {
            Kind::Integer(integer) => match integer.layout().size {
                1 if integer.is_signed(Target::X86_64) => "movsx rax, byte",
                1 => "movzx eax, byte",
                2 if integer.is_signed(Target::X86_64) => "movsx rax, word",
                2 => "movzx eax, word",
                4 if integer.is_signed(Target::X86_64) => "movsxd rax, dword",
                4 => "mov eax, dword",
                _ => "mov rax, qword",
            },
            Kind::Pointer(_) => "mov rax, qword",
            Kind::Float(Float::Single) => "movss xmm0,",
            Kind::Float(Float::Double) => "movsd xmm0,",
            Kind::Function { .. } | Kind::Array(..) | Kind::Record(_) => return,
            Kind::LongDouble => "fld tbyte",
            Kind::Void => unreachable!(),
        };
        writeln!(self.text, "    {instruction} {address}").unwrap();
    }

    fn store(&mut self, ty: &Type, address: &str) {
        if let Kind::Float(float) = ty.kind {
            writeln!(self.text, "    {} {address}, xmm0", float_move(float)).unwrap();
        } else if ty.kind == Kind::LongDouble {
            writeln!(self.text, "    fstp tbyte {address}").unwrap();
        } else {
            writeln!(self.text, "    mov qword {address}, rax").unwrap();
        }
    }

    fn normalize_integer(&mut self, ty: &Type) {
        let Kind::Integer(integer) = ty.kind else {
            return;
        };
        let signed = integer.is_signed(Target::X86_64);
        let instruction = match (integer.layout().size, signed) {
            (1, true) => "movsx rax, al",
            (1, false) => "movzx eax, al",
            (2, true) => "movsx rax, ax",
            (2, false) => "movzx eax, ax",
            (4, true) => "movsxd rax, eax",
            (4, false) => "mov eax, eax",
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
            (Kind::LongDouble, Kind::Void) => self.text.push_str("    fstp st(0)\n"),
            (Kind::Integer(integer), Kind::LongDouble) => self.integer_to_x87(*integer),
            (Kind::LongDouble, Kind::Integer(integer)) => self.x87_to_integer(*integer, dest),
            (Kind::Float(float), Kind::LongDouble) => self.sse_to_x87(*float),
            (Kind::LongDouble, Kind::Float(float)) => self.x87_to_sse(*float),
            (Kind::Float(src), Kind::Float(dst)) => {
                writeln!(
                    self.text,
                    "    cvt{}2{} xmm0, xmm0",
                    suffix(*src),
                    suffix(*dst)
                )
                .unwrap();
            }
            (Kind::Integer(integer), Kind::Float(float)) => {
                self.integer_to_float(*integer, *float);
            }
            (Kind::Float(float), Kind::Integer(integer)) => {
                self.float_to_integer(*float, *integer, dest);
            }
            (_, Kind::Integer(_)) => self.normalize_integer(dest),
            _ => {}
        }
    }

    fn integer_to_float(&mut self, integer: Integer, float: Float) {
        let suffix = suffix(float);
        if integer.layout().size == 8 && !integer.is_signed(Target::X86_64) {
            // For large unsigned inputs, convert half with the dropped bit
            // folded into its low bit, then double without a second rounding.
            let small = self.label();
            let end = self.label();
            writeln!(self.text, "    test rax, rax\n    jns {small}\n    mov rcx, rax\n    and ecx, 1\n    shr rax, 1\n    or rax, rcx\n    cvtsi2{suffix} xmm0, rax\n    add{suffix} xmm0, xmm0\n    jmp {end}\n{small}:\n    cvtsi2{suffix} xmm0, rax\n{end}:").unwrap();
        } else {
            writeln!(self.text, "    cvtsi2{suffix} xmm0, rax").unwrap();
        }
    }

    fn float_to_integer(&mut self, float: Float, integer: Integer, dest: &Type) {
        let suffix = suffix(float);
        if integer.layout().size == 8 && !integer.is_signed(Target::X86_64) {
            // Binary32 and binary64 both represent 2^63 exactly.
            let (bits, mask) = match float {
                Float::Single => (0x5f00_0000, 0x8000_0000_0000_0000u64),
                Float::Double => (0x43e0_0000_0000_0000, 0x8000_0000_0000_0000u64),
            };
            let small = self.label();
            let end = self.label();
            self.float_constant(bits, float, 1);
            writeln!(self.text, "    ucomi{suffix} xmm0, xmm1\n    jb {small}\n    sub{suffix} xmm0, xmm1\n    cvtt{suffix}2si rax, xmm0\n    mov rcx, 0x{mask:x}\n    or rax, rcx\n    jmp {end}\n{small}:").unwrap();
            writeln!(self.text, "    cvtt{suffix}2si rax, xmm0\n{end}:").unwrap();
        } else {
            writeln!(self.text, "    cvtt{suffix}2si rax, xmm0").unwrap();
            self.normalize_integer(dest);
        }
    }

    fn float_constant(&mut self, bits: u64, float: Float, register: usize) {
        let (mov, gp) = if float == Float::Single {
            ("movd", "eax")
        } else {
            ("movq", "rax")
        };
        writeln!(
            self.text,
            "    mov {gp}, 0x{bits:x}\n    {mov} xmm{register}, {gp}"
        )
        .unwrap();
    }

    fn label(&mut self) -> String {
        let label = format!(".L.bcc.{}", self.next_label);
        self.next_label += 1;
        label
    }

    fn adjust_stack(&mut self, instruction: &str, bytes: usize) {
        if bytes != 0 {
            writeln!(self.text, "    {instruction} rsp, {bytes}").unwrap();
        }
    }

    fn check_symbol(&self, name: &str, site: &Position) -> Result<(), Diagnostic> {
        if name.contains('\\') {
            return Err(Diagnostic::new(
                site,
                "code generation for universal characters in function names is not implemented",
            ));
        }
        Ok(())
    }

    fn use_symbol(&mut self, name: &'src str, site: &Position) -> Result<(), Diagnostic> {
        self.check_symbol(name, site)?;
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

    fn emit_string(&mut self, bytes: &[u8]) -> String {
        let label = format!(".L.str.{}", self.string_count);
        self.string_count += 1;
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
}

fn supported_abi_type(ty: &Type) -> bool {
    matches!(
        ty.kind,
        Kind::Integer(_) | Kind::Float(_) | Kind::LongDouble | Kind::Pointer(_) | Kind::Record(_)
    )
}

fn direct_stack_argument(ty: &Type) -> bool {
    matches!(ty.kind, Kind::Record(_) | Kind::LongDouble)
}

fn suffix(float: Float) -> &'static str {
    match float {
        Float::Single => "ss",
        Float::Double => "sd",
    }
}

fn float_move(float: Float) -> &'static str {
    match float {
        Float::Single => "movss",
        Float::Double => "movsd",
    }
}
