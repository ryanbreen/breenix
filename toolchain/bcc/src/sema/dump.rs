//! Write the cconf type record format from the typed translation unit.
use std::io::{self, Write};

use super::{Expression, ExpressionKind, Linkage, Position, Statement, TranslationUnit, Type};

impl TranslationUnit<'_> {
    pub fn write_types(&self, mut out: impl Write) -> io::Result<()> {
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
            Statement::Empty | Statement::Return(None) => {}
        }
    }
    Ok(())
}

fn expression(out: &mut impl Write, expr: &Expression<'_>) -> io::Result<()> {
    let (form, value) = match &expr.kind {
        ExpressionKind::Identifier(_) => {
            let category = if matches!(expr.ty.kind, crate::types::Kind::Function { .. }) {
                "function-designator"
            } else {
                "lvalue"
            };
            ("identifier", category)
        }
        ExpressionKind::Integer(_) => ("integer-constant", "rvalue"),
        ExpressionKind::String(_) => ("string-literal", "lvalue"),
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
