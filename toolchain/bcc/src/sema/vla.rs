//! N1570 §§6.2.4 and 6.7.6.2: automatic arrays with a runtime outer bound.

use super::*;

pub(super) struct Array {
    ty: Type,
    size: usize,
}

impl Array {
    pub(super) fn reference<'a>(
        &self,
        site: &'a Lexeme,
        local: usize,
        pointer: &Type,
    ) -> Expression<'a> {
        Expression {
            site,
            ty: self.ty.clone(),
            kind: ExpressionKind::Unary(
                Unary::Dereference,
                Box::new(local_value(site, local, pointer)),
            ),
        }
    }
}

impl<'a> FunctionBody<'a, '_> {
    pub(super) fn variable_array(
        &mut self,
        name: &'a Lexeme,
        ty: Type,
        alignment: Option<usize>,
        bound: parse::Expression<'a>,
    ) -> Result<Statement<'a>, Diagnostic> {
        let Kind::Array(element, None) = &ty.kind else {
            unreachable!("parser supplies a variable outer array bound");
        };
        let layout = element.complete_layout().unwrap();
        if alignment.unwrap_or(layout.align).max(layout.align) > 16 {
            return Err(name.error("over-aligned variable length arrays are unsupported"));
        }
        if !self.statement_regions.is_empty() {
            return Err(
                name.error("variable length arrays inside statement expressions are unsupported")
            );
        }
        if self.has_alloca && self.vla_locals.is_empty() {
            return Err(name.error("mixing alloca and variable length arrays is unsupported"));
        }
        let bound = self.expression(bound)?;
        if !matches!(bound.ty.kind, Kind::Integer(_)) {
            return Err(bound.site.error("variable array bound requires an integer"));
        }
        let word = Type::new(Kind::Integer(Integer::UnsignedLong));
        let mut stack_word = word.clone();
        // The two stack operations access the mark directly in the frame.
        stack_word.qualifiers.volatile = true;
        let mark = self.temporary(name, &stack_word);
        let size = self.temporary(name, &word);
        let pointer_type = ty.clone().pointer_to();
        let pointer = self.temporary(name, &pointer_type);
        if self
            .scopes
            .last_mut()
            .unwrap()
            .insert(&name.text, pointer)
            .is_some()
        {
            return Err(name.error("duplicate local declaration"));
        }
        self.vla_scopes.push(mark);
        self.vla_locals.insert(pointer, Array { ty, size });
        self.has_alloca = true;

        let byte_count = Expression {
            site: name,
            ty: word.clone(),
            kind: ExpressionKind::Binary {
                op: Binary::Multiply,
                left: Box::new(bound),
                right: Box::new(Expression {
                    site: name,
                    ty: word.clone(),
                    kind: ExpressionKind::Integer(layout.size as u64),
                }),
                left_type: word.clone(),
                right_type: word.clone(),
            },
        };
        let address = Expression {
            site: name,
            ty: Type::new(Kind::Void).pointer_to(),
            kind: ExpressionKind::Alloca(Box::new(local_value(name, size, &word))),
        };
        Ok(Statement::Block(vec![
            Statement::SaveVlaStack(mark),
            initialize(size, word, byte_count),
            initialize(pointer, pointer_type, address),
        ]))
    }

    pub(super) fn vla_size(&self, expr: &Expression<'a>) -> Option<usize> {
        let ExpressionKind::Unary(Unary::Dereference, pointer) = &expr.kind else {
            return None;
        };
        let ExpressionKind::Local(id) = pointer.kind else {
            return None;
        };
        self.vla_locals.get(&id).map(|array| array.size)
    }
}

fn local_value<'a>(site: &'a Lexeme, index: usize, ty: &Type) -> Expression<'a> {
    Expression {
        site,
        ty: ty.clone(),
        kind: ExpressionKind::Local(index),
    }
}

fn initialize<'a>(local: usize, ty: Type, expr: Expression<'a>) -> Statement<'a> {
    Statement::Initialize {
        local,
        initializer: ObjectInitializer {
            zero_bytes: 0,
            values: vec![Initializer {
                offset: 0,
                ty,
                bit_field: None,
                value: InitValue::Expression(expr),
            }],
        },
    }
}
