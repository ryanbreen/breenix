//! Write the cconf type record format from the typed translation unit.
use std::io::{self, Write};

use super::{Expression, ExpressionKind, Linkage, Position, Statement, TranslationUnit, Type};

impl TranslationUnit<'_> {
    pub fn write_types(&self, mut out: impl Write) -> io::Result<()> {
        for object in &self.objects {
            declaration(
                &mut out,
                object.site,
                object.name,
                &object.ty,
                "external",
                false,
            )?;
        }
        for function in &self.functions {
            let linkage = match function.linkage {
                Linkage::Internal => "internal",
                Linkage::External => "external",
            };
            declaration(
                &mut out,
                function.site,
                function.name,
                &function.ty,
                linkage,
                function.body.is_some(),
            )?;
            for param in &function.params {
                if let Some((name, site)) = param.name {
                    declaration(&mut out, site, name, &param.ty, "none", false)?;
                }
            }
            for local in function.locals.iter().skip(function.params.len()) {
                let obj = &local.object;
                declaration(&mut out, obj.site, obj.name, &obj.ty, "none", true)?;
            }
            if let Some(body) = &function.body {
                statements(&mut out, body)?;
            }
        }
        Ok(())
    }
}

fn start(out: &mut impl Write, what: &str, site: &Position) -> io::Result<()> {
    write!(
        out,
        "{{\"what\":\"{what}\",\"line\":{},\"col\":{}",
        site.loc.line, site.loc.column
    )
}

fn declaration(
    out: &mut impl Write,
    site: &Position,
    name: &str,
    ty: &Type,
    linkage: &str,
    definition: bool,
) -> io::Result<()> {
    start(out, "decl", site)?;
    out.write_all(b",\"name\":")?;
    string(out, name)?;
    out.write_all(b",\"type\":")?;
    string(out, &ty.to_string())?;
    writeln!(
        out,
        ",\"linkage\":\"{linkage}\",\"definition\":{definition}}}"
    )
}

fn statements(out: &mut impl Write, body: &[Statement<'_>]) -> io::Result<()> {
    for statement in body {
        match statement {
            Statement::Block(inner) => statements(out, inner)?,
            Statement::Expression(expr) | Statement::Return(Some(expr)) => expression(out, expr)?,
            Statement::Initialize { values, .. } => {
                for initializer in values {
                    expression(out, &initializer.value)?;
                }
            }
            Statement::If { condition, yes, no } => {
                expression(out, condition)?;
                statements(out, std::slice::from_ref(yes))?;
                if let Some(no) = no {
                    statements(out, std::slice::from_ref(no))?;
                }
            }
            Statement::Loop {
                condition,
                increment,
                body,
                ..
            } => {
                if let Some(condition) = condition {
                    expression(out, condition)?;
                }
                if let Some(increment) = increment {
                    expression(out, increment)?;
                }
                statements(out, std::slice::from_ref(body))?;
            }
            Statement::Switch { value, body, .. } => {
                expression(out, value)?;
                statements(out, std::slice::from_ref(body))?;
            }
            Statement::Label(_, body) => statements(out, std::slice::from_ref(body))?,
            Statement::Empty | Statement::Return(None) | Statement::Jump(_) => {}
        }
    }
    Ok(())
}

fn expression(out: &mut impl Write, expr: &Expression<'_>) -> io::Result<()> {
    let (form, value) = match &expr.kind {
        ExpressionKind::Identifier(_) | ExpressionKind::Local(_) => {
            let category = if matches!(expr.ty.kind, crate::types::Kind::Function { .. }) {
                "function-designator"
            } else {
                "lvalue"
            };
            ("identifier", category)
        }
        ExpressionKind::Integer(_) => ("integer-constant", "rvalue"),
        ExpressionKind::Float(_) => ("floating-constant", "rvalue"),
        ExpressionKind::Unary(op, operand) => {
            expression(out, operand)?;
            let value = if *op == super::Unary::Dereference {
                "lvalue"
            } else {
                "rvalue"
            };
            ("unary", value)
        }
        ExpressionKind::Binary { left, right, .. } => {
            expression(out, left)?;
            expression(out, right)?;
            ("binary", "rvalue")
        }
        ExpressionKind::Cast(operand) => {
            expression(out, operand)?;
            ("cast", "rvalue")
        }
        ExpressionKind::Conditional { condition, yes, no } => {
            expression(out, condition)?;
            expression(out, yes)?;
            expression(out, no)?;
            ("conditional", "rvalue")
        }
        ExpressionKind::Assignment { left, right, .. } => {
            expression(out, left)?;
            expression(out, right)?;
            ("assignment", "rvalue")
        }
        ExpressionKind::Update { operand, .. } => {
            expression(out, operand)?;
            ("update", "rvalue")
        }
        ExpressionKind::Member { base, .. } => {
            expression(out, base)?;
            ("member", if expr.lvalue() { "lvalue" } else { "rvalue" })
        }
        ExpressionKind::String(_) => ("string-literal", "lvalue"),
        ExpressionKind::VaStart(list) | ExpressionKind::VaArg(list) | ExpressionKind::VaEnd(list) => {
            expression(out, list)?;
            (expr.site.text.as_ref(), "rvalue")
        }
        ExpressionKind::VaCopy { dest, source } => {
            expression(out, dest)?;
            expression(out, source)?;
            ("__builtin_va_copy", "rvalue")
        }
        ExpressionKind::Call { callee, arguments } => {
            expression(out, callee)?;
            for argument in arguments {
                expression(out, &argument.expr)?;
            }
            ("call", "rvalue")
        }
    };
    start(out, "expr", expr.position())?;
    write!(out, ",\"form\":\"{form}\",\"value\":\"{value}\",\"type\":")?;
    string(out, &expr.ty.to_string())?;
    if let Some(ty) = expr.converted_type() {
        out.write_all(b",\"converted\":")?;
        string(out, &ty.to_string())?;
    }
    if let ExpressionKind::Call { arguments, .. } = &expr.kind {
        out.write_all(b",\"args\":[")?;
        for (index, argument) in arguments.iter().enumerate() {
            if index != 0 {
                out.write_all(b",")?;
            }
            string(out, &argument.ty.to_string())?;
        }
        out.write_all(b"]")?;
    }
    out.write_all(b"}\n")
}

fn string(out: &mut impl Write, value: &str) -> io::Result<()> {
    out.write_all(b"\"")?;
    for ch in value.chars() {
        match ch {
            '"' | '\\' => write!(out, "\\{ch}")?,
            '\0'..='\u{1f}' => write!(out, "\\u{:04x}", u32::from(ch))?,
            _ => write!(out, "{ch}")?,
        }
    }
    out.write_all(b"\"")
}
