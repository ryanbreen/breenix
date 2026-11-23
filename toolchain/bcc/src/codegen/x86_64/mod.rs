//! Unoptimized SysV AMD64 assembly in bas's Intel-style syntax.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use crate::pp::{Diagnostic, Position, Target};
use crate::sema::{
    Argument, Binary, Expression, ExpressionKind, Function, Linkage, Statement, TranslationUnit,
    Unary,
};
use crate::types::{Float, Integer, Kind, Type};

mod flow;

use flow::Slot;

const INTEGER_REGS: [&str; 6] = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"];

#[derive(Clone, Copy)]
enum Place {
    Integer(&'static str),
    Vector(usize, Float),
    Stack(usize),
}

#[derive(Default)]
struct CallLayout {
    integers: usize,
    vectors: usize,
    stack: usize,
}

impl CallLayout {
    // AMD64 psABI §3.2.3. Scalar INTEGER and SSE arguments consume
    // registers independently and fall back to ordered stack eightbytes.
    fn place(&mut self, ty: &Type) -> Place {
        match ty.kind {
            Kind::Float(float) if self.vectors < 8 => {
                let index = self.vectors;
                self.vectors += 1;
                return Place::Vector(index, float);
            }
            Kind::Integer(_) | Kind::Pointer(_) if self.integers < INTEGER_REGS.len() => {
                let reg = INTEGER_REGS[self.integers];
                self.integers += 1;
                return Place::Integer(reg);
            }
            _ => {}
        }
        let offset = self.stack;
        self.stack += 8;
        Place::Stack(offset)
    }
}

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
        data: String::new(),
        externs: BTreeSet::new(),
        text: String::from("section .text\n"),
        rodata: String::new(),
        string_count: 0,
        next_label: 0,
    };
    for function in &unit.functions {
        if let Some(body) = &function.body {
            emitter.function(function, body)?;
        }
    }
    for name in emitter.externs {
        writeln!(emitter.text, "extern {name}").unwrap();
    }
    if !emitter.rodata.is_empty() {
        emitter.text.push_str("\nsection .rodata\n");
        emitter.text.push_str(&emitter.rodata);
    }
    if !emitter.data.is_empty() {
        emitter.text.push_str("\nsection .bss\n");
        emitter.text.push_str(&emitter.data);
    }
    Ok(emitter.text)
}

struct Emitter<'unit, 'src> {
    functions: HashMap<&'src str, &'unit Function<'src>>,
    locals: Vec<Slot>,
    flow_labels: HashMap<usize, String>,
    data: String,
    externs: BTreeSet<&'src str>,
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
        let Kind::Function { result, .. } = &function.ty.kind else {
            unreachable!()
        };
        if result.kind != Kind::Void && !abi_scalar(result) {
            return Err(Diagnostic::new(
                function.site,
                "function result is outside the scalar ABI subset",
            ));
        }
        for param in &function.params {
            if !abi_scalar(&param.ty) {
                return Err(Diagnostic::new(
                    param.name.unwrap().1,
                    "parameter is outside the scalar ABI subset",
                ));
            }
        }
        let frame = self.layout(function)?;
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
        self.adjust_stack("sub", frame);

        let mut layout = CallLayout::default();
        for (index, param) in function.params.iter().enumerate() {
            let memory = self.locals[index].memory(0);
            match layout.place(&param.ty) {
                Place::Integer(reg) => {
                    writeln!(self.text, "    mov qword {memory}, {reg}").unwrap()
                }
                Place::Vector(reg, float) => {
                    let instruction = float_move(float);
                    writeln!(self.text, "    {instruction} {memory}, xmm{reg}").unwrap();
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
        self.statements(body, result, &epilogue)?;
        if name == "main"
            && function.linkage == Linkage::External
            && result.kind == Kind::Integer(Integer::Int)
        {
            // N1570 §5.1.2.2.3 specifies the result when int main falls through.
            self.text.push_str("    mov eax, 0\n");
        }
        writeln!(
            self.text,
            "{epilogue}:\n    leave\n    ret\n.size {name}, . - {name}"
        )
        .unwrap();
        Ok(())
    }

    // Canonical value locations: a normalized integer or pointer in RAX,
    // a float of its declared width in XMM0, or a record's address in RAX.
    fn value(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        if matches!(expr.ty.kind, Kind::LongDouble) {
            return Err(Diagnostic::new(
                expr.position(),
                "value is outside the scalar code generation subset",
            ));
        }
        match &expr.kind {
            ExpressionKind::Integer(value) => {
                writeln!(self.text, "    mov rax, 0x{value:x}").unwrap();
                self.normalize_integer(&expr.ty);
            }
            ExpressionKind::Float(bits) => {
                let Kind::Float(float) = expr.ty.kind else {
                    unreachable!()
                };
                self.float_constant(*bits, float, 0);
            }
            ExpressionKind::String(_)
            | ExpressionKind::Identifier(_)
            | ExpressionKind::Local(_)
            | ExpressionKind::Member { .. } => {
                self.address(expr)?;
                self.load(&expr.ty, "[rax]");
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
            ExpressionKind::Call { callee, arguments } => {
                self.call(callee, arguments)?;
                self.normalize_integer(&expr.ty);
            }
        }
        Ok(())
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
    // expression stack. Consume that slot and leave the result in RAX/XMM0.
    fn combine(&mut self, op: Binary, left: &Type, right: &Type, result: &Type) {
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
        let mut layout = CallLayout::default();
        let places: Vec<_> = arguments.iter().map(|arg| layout.place(&arg.ty)).collect();
        let callee_slot = layout.stack + arguments.len() * 8;
        let reserve = (callee_slot + usize::from(direct.is_none()) * 8).next_multiple_of(16);
        self.adjust_stack("sub", reserve);
        if direct.is_none() {
            self.value(callee)?;
            writeln!(self.text, "    mov qword [rsp + {callee_slot}], rax").unwrap();
        }
        // Stage the values after the outgoing stack area, where nested
        // calls cannot clobber them or the final argument registers.
        for (index, arg) in arguments.iter().enumerate() {
            self.converted(&arg.expr, &arg.ty)?;
            let offset = layout.stack + index * 8;
            self.store(&arg.ty, &format!("[rsp + {offset}]"));
        }
        for (index, place) in places.into_iter().enumerate() {
            let offset = layout.stack + index * 8;
            match place {
                Place::Integer(reg) => {
                    writeln!(self.text, "    mov {reg}, qword [rsp + {offset}]").unwrap();
                }
                Place::Vector(reg, float) => {
                    let instruction = float_move(float);
                    writeln!(self.text, "    {instruction} xmm{reg}, [rsp + {offset}]").unwrap();
                }
                Place::Stack(slot) => {
                    let (reg, size) = match arguments[index].ty.kind {
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
        Ok(())
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
            Kind::Void | Kind::LongDouble => unreachable!(),
        };
        writeln!(self.text, "    {instruction} {address}").unwrap();
    }

    fn store(&mut self, ty: &Type, address: &str) {
        if let Kind::Float(float) = ty.kind {
            writeln!(self.text, "    {} {address}, xmm0", float_move(float)).unwrap();
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

fn abi_scalar(ty: &Type) -> bool {
    matches!(
        ty.kind,
        Kind::Integer(_) | Kind::Float(_) | Kind::Pointer(_)
    )
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
