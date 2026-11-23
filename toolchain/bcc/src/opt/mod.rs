//! Conservative optimization of typed function bodies before either backend.

mod constant;
mod cse;
mod unreachable;

use crate::pp::Target;
use crate::sema::{
    Expression, ExpressionKind, InitValue, LocalStorage, ObjectInitializer, Statement,
    TranslationUnit, Unary,
};
use crate::types::Kind;

pub(crate) fn run(unit: &mut TranslationUnit<'_>, target: Target) {
    let preserve_fp = unit.preserves_float_environment();
    for function in &mut unit.functions {
        let Some(body) = &mut function.body else {
            continue;
        };
        // N1570 5.1.2.3 and 6.5.3.2. Only direct reads of private integer
        // locals are candidates. An address taken anywhere can escape.
        let mut eligible: Vec<_> = function
            .locals
            .iter()
            .map(|local| {
                let ty = &local.object.ty;
                matches!(local.storage, LocalStorage::Automatic)
                    && matches!(ty.kind, Kind::Integer(_))
                    && !ty.qualifiers.volatile
                    && !ty.qualifiers.atomic
            })
            .collect();
        walk_blocks(body, &mut EscapeAnalysis(&mut eligible));
        constant::run(body, &eligible, target, preserve_fp);
        unreachable::run(body);
        cse::run(body, &mut function.locals, &eligible);
    }
}

/// Walk statements in emission order, reporting boundaries where a
/// straight-line fact cannot in general be carried forward.
trait BlockPass<'a> {
    fn boundary(&mut self) {}
    fn expression(&mut self, expr: &mut Expression<'a>);
    fn initialize(&mut self, _local: usize, initializer: &mut ObjectInitializer<'a>) {
        initializer_expressions(initializer, |expr| self.expression(expr));
    }
}

fn walk_blocks<'a>(body: &mut [Statement<'a>], pass: &mut impl BlockPass<'a>) {
    use std::slice::from_mut;
    for statement in body {
        match statement {
            Statement::Block(inner) => walk_blocks(inner, pass),
            Statement::Initialize { local, initializer } => pass.initialize(*local, initializer),
            Statement::Expression(expr) => pass.expression(expr),
            Statement::If { condition, yes, no } => {
                pass.expression(condition);
                pass.boundary();
                walk_blocks(from_mut(yes.as_mut()), pass);
                pass.boundary();
                if let Some(no) = no {
                    walk_blocks(from_mut(no.as_mut()), pass);
                    pass.boundary();
                }
            }
            Statement::Loop {
                condition,
                increment,
                body,
                ..
            } => {
                pass.boundary();
                if let Some(condition) = condition {
                    pass.expression(condition);
                    pass.boundary();
                }
                walk_blocks(from_mut(body.as_mut()), pass);
                pass.boundary();
                if let Some(increment) = increment {
                    pass.expression(increment);
                    pass.boundary();
                }
            }
            Statement::Switch { value, body, .. } => {
                pass.expression(value);
                pass.boundary();
                walk_blocks(from_mut(body.as_mut()), pass);
                pass.boundary();
            }
            Statement::Label(_, body) => {
                pass.boundary();
                walk_blocks(from_mut(body.as_mut()), pass);
            }
            Statement::Return(expr) => {
                if let Some(expr) = expr {
                    pass.expression(expr);
                }
                pass.boundary();
            }
            Statement::Jump(_) => pass.boundary(),
            Statement::Empty => {}
        }
    }
}

struct EscapeAnalysis<'f>(&'f mut [bool]);

impl<'a> BlockPass<'a> for EscapeAnalysis<'_> {
    fn expression(&mut self, expr: &mut Expression<'a>) {
        if let ExpressionKind::Unary(Unary::Address, operand) = &expr.kind {
            if let ExpressionKind::Local(local) = operand.kind {
                self.0[local] = false;
            }
        }
        if let ExpressionKind::StatementExpression {
            statements, value, ..
        } = &mut expr.kind
        {
            walk_blocks(statements, self);
            if let Some(value) = value {
                self.expression(value);
            }
        } else {
            children(expr, |child| self.expression(child));
        }
    }
}

fn initializer_expressions<'a>(
    initializer: &mut ObjectInitializer<'a>,
    mut visit: impl FnMut(&mut Expression<'a>),
) {
    for init in &mut initializer.values {
        if let InitValue::Expression(expr) = &mut init.value {
            visit(expr);
        }
    }
}

fn children<'a>(expr: &mut Expression<'a>, mut visit: impl FnMut(&mut Expression<'a>)) {
    use ExpressionKind::*;
    match &mut expr.kind {
        Unary(_, expr)
        | Cast(expr)
        | Update { operand: expr, .. }
        | Member { base: expr, .. }
        | VaStart(expr)
        | VaArg(expr)
        | VaEnd(expr)
        | FloatClass { operand: expr, .. }
        | SignBit(expr)
        | Alloca(expr) => visit(expr),
        Binary { left, right, .. }
        | Comma(left, right)
        | Assignment { left, right, .. }
        | VaCopy {
            dest: left,
            source: right,
        } => {
            visit(left);
            visit(right);
        }
        OmittedConditional {
            value, fallback, ..
        } => {
            visit(value);
            visit(fallback);
        }
        Conditional { condition, yes, no } => {
            visit(condition);
            visit(yes);
            visit(no);
        }
        CompoundLiteral { initializer, .. } => initializer_expressions(initializer, visit),
        Atomic { arguments, .. } => {
            for argument in arguments {
                visit(&mut argument.expr);
            }
        }
        Call { callee, arguments } => {
            visit(callee);
            for argument in arguments {
                visit(&mut argument.expr);
            }
        }
        Identifier(_)
        | ThreadLocal(_)
        | Local(_)
        | StaticLiteral(_)
        | Integer(_)
        | Float(_)
        | String(_)
        | FltRounds
        | StatementExpression { .. } => {}
    }
}
