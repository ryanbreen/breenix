//! N1570 6.5.1.1, 6.5.3.4, 6.7.2.1p13, 6.7.10 and 7.19.

use super::*;

pub(crate) fn type_alignment(ty: &Type, site: &Lexeme) -> Result<usize, Diagnostic> {
    ty.layout()
        .filter(|layout| layout.size != 0)
        .map(|layout| layout.align)
        .ok_or_else(|| site.error("_Alignof requires a complete object type"))
}

pub(crate) fn check_static_assertion(
    site: &Lexeme,
    message: &str,
    value: u64,
) -> Result<(), Diagnostic> {
    if value == 0 {
        return Err(site.error(format!("static assertion failed: {message}")));
    }
    Ok(())
}

impl<'a> FunctionBody<'a, '_> {
    pub(super) fn static_assertion(
        &mut self,
        assertion: parse::StaticAssertion<'a>,
    ) -> Result<(), Diagnostic> {
        let expr = self.unevaluated(assertion.expression)?;
        let value = body::constant_integer(&expr, self.target)
            .ok_or_else(|| expr.site.error("expected an integer constant expression"))?;
        check_static_assertion(assertion.site, &assertion.message, value)
    }

    pub(super) fn unevaluated(
        &mut self,
        expr: parse::Expression<'a>,
    ) -> Result<Expression<'a>, Diagnostic> {
        let locals = self.locals.len();
        let literals = self.static_literals.as_ref().map(|(_, saved)| saved.len());
        let result = self.expression(expr);
        self.locals.truncate(locals);
        if let Some((_, saved)) = &mut self.static_literals {
            saved.truncate(literals.unwrap());
        }
        result
    }

    pub(super) fn generic(
        &mut self,
        site: &'a Lexeme,
        control: parse::Expression<'a>,
        associations: Vec<parse::Association<'a>>,
    ) -> Result<Expression<'a>, Diagnostic> {
        let control = self.unevaluated(control)?.value_type();
        let mut types: Vec<&Type> = Vec::new();
        let mut matching = None;
        let mut default = None;
        for (index, association) in associations.iter().enumerate() {
            if let Some(ty) = &association.ty {
                if ty.layout().is_none_or(|layout| layout.size == 0) {
                    return Err(association
                        .site
                        .error("generic association requires a complete object type"));
                }
                if types.iter().any(|prior| prior.compatible(ty)) {
                    return Err(association
                        .site
                        .error("duplicate compatible generic association"));
                }
                if control.compatible(ty) && matching.replace(index).is_some() {
                    return Err(association
                        .site
                        .error("multiple generic associations match the controlling type"));
                }
                types.push(ty);
            } else if default.replace(index).is_some() {
                return Err(association
                    .site
                    .error("duplicate default generic association"));
            }
        }
        let selected = matching
            .or(default)
            .ok_or_else(|| site.error(format!("no matching generic association for {control}")))?;
        let mut result = None;
        for (index, association) in associations.into_iter().enumerate() {
            if index == selected {
                result = Some(self.expression(association.expression)?);
            } else {
                self.unevaluated(association.expression)?;
            }
        }
        // Returning the selected expression itself also preserves its
        // value category, bit-field metadata, and constant-expression form.
        Ok(result.unwrap())
    }

    pub(super) fn record_member(
        &self,
        base: &Type,
        name: &Lexeme,
    ) -> Result<(usize, Type, Option<BitField>), Diagnostic> {
        let Kind::Record(record) = base.kind else {
            return Err(name.error("member access requires a struct or union"));
        };
        if record.layout.is_none() {
            return Err(name.error("member access requires a complete record type"));
        }
        let records = self.records.records;
        let definition = records[record.id].definition.as_ref().unwrap();
        let path = definition
            .member_path(&name.text, records)
            .ok_or_else(|| name.error(format!("no member named '{}'", name.text)))?;
        let mut ty = base.clone();
        let mut offset = 0;
        let mut bit_field = None;
        for index in path {
            let Kind::Record(record) = ty.kind else {
                unreachable!()
            };
            let member = &records[record.id].definition.as_ref().unwrap().members[index];
            offset += member.offset;
            bit_field = member.bit_field;
            let qualifiers = ty.qualifiers;
            ty = member.ty.clone();
            ty.qualify_member(qualifiers);
        }
        Ok((offset, ty, bit_field))
    }

    // The compiler support header exposes N1570 7.19's offsetof through
    // this builtin. A designator can follow nested and anonymous members.
    pub(super) fn offsetof(
        &mut self,
        site: &Lexeme,
        mut ty: Type,
        path: Vec<parse::Designator<'a>>,
    ) -> Result<usize, Diagnostic> {
        if !matches!(ty.kind, Kind::Record(_)) {
            return Err(site.error("offsetof requires a structure or union type"));
        }
        let mut offset = 0usize;
        for designator in path {
            let (part, next) = match designator {
                parse::Designator::Member(name) => {
                    let (part, ty, field) = self.record_member(&ty, name)?;
                    if field.is_some() {
                        return Err(name.error("offsetof cannot designate a bit-field"));
                    }
                    (part, ty)
                }
                parse::Designator::Index(index) => {
                    let index = self.unevaluated(index)?;
                    let Kind::Array(element, _) = ty.kind else {
                        return Err(index.site.error("offsetof subscript requires an array"));
                    };
                    let part = body::constant_integer(&index, self.target)
                        .filter(|&value| {
                            !matches!(index.ty.kind, Kind::Integer(integer)
                                if integer.is_signed(self.target) && (value as i64) < 0)
                        })
                        .and_then(|value| usize::try_from(value).ok())
                        .and_then(|value| value.checked_mul(element.layout()?.size))
                        .ok_or_else(|| index.site.error("offsetof requires a nonnegative constant subscript within the addressable range"))?;
                    (part, *element)
                }
            };
            offset = offset
                .checked_add(part)
                .ok_or_else(|| site.error("offsetof exceeds the target address space"))?;
            ty = next;
        }
        Ok(offset)
    }
}
