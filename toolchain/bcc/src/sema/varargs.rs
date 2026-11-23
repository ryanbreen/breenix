//! N1570 §7.16.1 variable argument list access macros.

use super::*;
use crate::types::{aarch64_va_list, amd64_va_list};

impl<'a> FunctionBody<'a, '_> {
    fn va_operand(
        &mut self,
        operand: parse::Expression<'a>,
        writable: bool,
    ) -> Result<Box<Expression<'a>>, Diagnostic> {
        let operand = self.expression(operand)?;
        let is_list = |ty: &Type, name| {
            matches!(ty.kind, Kind::Record(record)
                if self.records.records.get(record.id).is_some_and(|entry|
                    entry.tag == Some(name)))
        };
        let valid = match self.target {
            Target::Aarch64 => {
                is_list(&operand.ty, aarch64_va_list::NAME)
                    && (!writable || operand.modifiable(self.records.records))
            }
            // Array objects decay just as va_list parameters adjust to pointers.
            Target::X86_64 => matches!(operand.value_type().kind, Kind::Pointer(element)
                if is_list(&element, amd64_va_list::NAME)
                    && (!writable || !element.qualifiers.constant)),
        };
        if valid {
            return Ok(Box::new(operand));
        }
        Err(operand.site.error(if writable {
            "expected a writable va_list"
        } else {
            "expected a va_list"
        }))
    }

    pub(super) fn va_arg(
        &mut self,
        site: &Lexeme,
        list: parse::Expression<'a>,
        ty: Type,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
        let list = self.va_operand(list, true)?;
        match ty.kind {
            Kind::Integer(integer) if integer == integer.promoted() => {}
            Kind::Pointer(_) | Kind::Float(Float::Double) | Kind::LongDouble => {}
            Kind::Record(record) if record.layout.is_some() => {}
            _ => return Err(site.error("va_arg requires a complete argument type after promotion")),
        }
        Ok((ExpressionKind::VaArg(list), ty.unqualified()))
    }

    pub(super) fn va_control(
        &mut self,
        site: &'a Lexeme,
        arguments: Vec<parse::Expression<'a>>,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
        let name = site.text.as_ref();
        let arity = if name == "__builtin_va_end" { 1 } else { 2 };
        if arguments.len() != arity {
            return Err(site.error(format!("{name} requires {arity} argument(s)")));
        }
        let mut args = arguments.into_iter();
        let list = self.va_operand(args.next().unwrap(), true)?;
        let kind = match name {
            "__builtin_va_start" => {
                let last = self.expression(args.next().unwrap())?;
                if !matches!(
                    last.kind,
                    ExpressionKind::Local(id) if Some(id) == self.last_named_parameter
                ) {
                    return Err(last
                        .site
                        .error("va_start requires the last named variadic parameter"));
                }
                let stable_type = match last.ty.kind {
                    Kind::Integer(integer) => integer.promoted() == integer,
                    Kind::Float(Float::Single) => false,
                    _ => true,
                };
                if !stable_type {
                    return Err(last.site.error(
                        "va_start parameter type changes under default argument promotions",
                    ));
                }
                ExpressionKind::VaStart(list)
            }
            "__builtin_va_copy" => ExpressionKind::VaCopy {
                dest: list,
                source: self.va_operand(args.next().unwrap(), false)?,
            },
            "__builtin_va_end" => ExpressionKind::VaEnd(list),
            _ => unreachable!(),
        };
        Ok((kind, Type::new(Kind::Void)))
    }
}
