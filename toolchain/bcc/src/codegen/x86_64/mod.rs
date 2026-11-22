//! AMD64 assembly for the hello subset, in bas's Intel-style syntax.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use crate::pp::Diagnostic;
use crate::sema::{
    Argument, Expression, ExpressionKind, Function, Linkage, Statement, TranslationUnit,
};
use crate::types::{Integer, Kind};

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
        externs: BTreeSet::new(),
        text: String::from("section .text\n"),
        rodata: String::new(),
        string_count: 0,
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
    Ok(emitter.text)
}

struct Emitter<'unit, 'src> {
    functions: HashMap<&'src str, &'unit Function<'src>>,
    externs: BTreeSet<&'src str>,
    text: String,
    rodata: String,
    string_count: usize,
}

impl<'src> Emitter<'_, 'src> {
    fn function(
        &mut self,
        function: &Function<'src>,
        body: &[Statement<'src>],
    ) -> Result<(), Diagnostic> {
        let name = function.name;
        if name.contains('\\') {
            return Err(Diagnostic::new(
                function.site,
                "code generation for universal characters in function names is not implemented",
            ));
        }
        let result = match &function.ty.kind {
            Kind::Function { result, params }
                if matches!(result.kind, Kind::Integer(Integer::Int) | Kind::Void)
                    && params.is_empty() =>
            {
                result
            }
            _ => return Err(Diagnostic::new(
                function.site,
                "code generation requires definitions with no parameters and an int or void result",
            )),
        };

        self.text.push('\n');
        if function.linkage == Linkage::External {
            writeln!(self.text, "global {name}").unwrap();
        }
        writeln!(self.text, ".type {name}, @function\n{name}:").unwrap();
        // AMD64 psABI §3.2.2: RSP + 8 is aligned to 16 on entry. Saving RBP
        // aligns RSP for calls and the frame uses no red-zone storage.
        self.text.push_str("    push rbp\n    mov rbp, rsp\n");
        let returned = self.statements(body)?;
        if !returned
            && name == "main"
            && function.linkage == Linkage::External
            && result.kind == Kind::Integer(Integer::Int)
        {
            // N1570 §5.1.2.2.3 specifies the result when int main falls through.
            self.text.push_str("    mov eax, 0\n");
        }
        writeln!(self.text, "    leave\n    ret\n.size {name}, . - {name}").unwrap();
        Ok(())
    }

    fn statements(&mut self, body: &[Statement<'src>]) -> Result<bool, Diagnostic> {
        for statement in body {
            match statement {
                Statement::Block(inner) => {
                    if self.statements(inner)? {
                        return Ok(true);
                    }
                }
                Statement::Empty => {}
                Statement::Expression(expr) => match &expr.kind {
                    ExpressionKind::Integer(_) | ExpressionKind::String(_) => {}
                    ExpressionKind::Call { callee, arguments } => self.call(callee, arguments)?,
                    _ => {
                        return Err(Diagnostic::new(
                            expr.position(),
                            "unsupported expression in AMD64 code generation",
                        ))
                    }
                },
                Statement::Return(value) => {
                    if let Some(expr) = value {
                        self.return_value(expr)?;
                    }
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn return_value(&mut self, expr: &Expression<'src>) -> Result<(), Diagnostic> {
        // AMD64 psABI §3.2.3: an int result occupies the low 32 bits of RAX.
        match &expr.kind {
            ExpressionKind::Integer(value) if i32::try_from(*value).is_ok() => {
                writeln!(self.text, "    mov eax, {value}").unwrap();
                Ok(())
            }
            ExpressionKind::Call { callee, arguments }
                if expr.ty.kind == Kind::Integer(Integer::Int) =>
            {
                self.call(callee, arguments)
            }
            _ => Err(Diagnostic::new(
                expr.position(),
                "unsupported return value in AMD64 code generation",
            )),
        }
    }

    fn call(
        &mut self,
        callee: &Expression<'src>,
        arguments: &[Argument<'src>],
    ) -> Result<(), Diagnostic> {
        let ExpressionKind::Identifier(name) = callee.kind else {
            return Err(Diagnostic::new(
                callee.position(),
                "AMD64 code generation requires a direct call",
            ));
        };
        let Some(function) = self.functions.get(name) else {
            return Err(Diagnostic::new(
                callee.position(),
                "AMD64 code generation requires a direct call",
            ));
        };
        if !matches!(callee.ty.kind, Kind::Function { .. }) || name.contains('\\') {
            return Err(Diagnostic::new(
                callee.position(),
                "unsupported call target in AMD64 code generation",
            ));
        }
        if function.body.is_none() {
            if function.linkage == Linkage::Internal {
                return Err(Diagnostic::new(
                    callee.position(),
                    "called static function has no definition",
                ));
            }
            self.externs.insert(name);
        }

        match arguments {
            [] => {}
            [argument] if matches!(argument.ty.kind, Kind::Pointer(_)) => {
                let ExpressionKind::String(bytes) = &argument.expr.kind else {
                    return Err(Diagnostic::new(
                        argument.expr.position(),
                        "AMD64 code generation requires a string literal argument",
                    ));
                };
                let label = self.emit_string(bytes);
                // AMD64 psABI §3.2.3: the first INTEGER-class argument goes in RDI.
                writeln!(self.text, "    lea rdi, [rip + {label}]").unwrap();
            }
            _ => {
                return Err(Diagnostic::new(
                    callee.position(),
                    "AMD64 code generation supports at most one string literal argument",
                ))
            }
        }
        writeln!(self.text, "    call {name}@plt").unwrap();
        Ok(())
    }

    fn emit_string(&mut self, bytes: &[u8]) -> String {
        let label = format!(".L.str.{}", self.string_count);
        self.string_count += 1;
        writeln!(self.rodata, "{label}:").unwrap();
        // The typed AST already contains the decoded bytes and terminating NUL.
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
