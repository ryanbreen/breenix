//! Forward constant propagation within straight-line regions.

use super::{children, initializer_expressions, walk_blocks, BlockPass};
use crate::pp::Target;
use crate::sema::Binary::{BitAnd, LogicalAnd, LogicalOr};
use crate::sema::Unary::Address;
use crate::sema::{canonical, constant_integer, Expression, ExpressionKind, InitValue};
use crate::sema::{ObjectInitializer, Statement};
use crate::types::Kind;

pub(super) fn run(
    body: &mut [Statement<'_>],
    eligible: &[bool],
    target: Target,
    preserve_fp: bool,
) {
    walk_blocks(
        body,
        &mut Propagate {
            values: vec![None; eligible.len()],
            eligible,
            target,
            preserve_fp,
        },
    );
}

struct Propagate<'f> {
    values: Vec<Option<u64>>,
    eligible: &'f [bool],
    target: Target,
    preserve_fp: bool,
}

impl<'a> BlockPass<'a> for Propagate<'_> {
    fn boundary(&mut self) {
        self.values.fill(None);
    }

    fn expression(&mut self, expr: &mut Expression<'a>) {
        // A standalone assignment has a known destination and no other
        // operands that can observe this write before the full expression ends.
        if let ExpressionKind::Assignment {
            left,
            right,
            operation: None,
        } = &mut expr.kind
        {
            if let ExpressionKind::Local(local) = left.kind {
                self.expression(right);
                if self.eligible[local] {
                    self.values[local] =
                        literal(right).map(|value| canonical(value, &left.ty, self.target));
                }
                return;
            }
        }

        // A nested store can be evaluated before or after a sibling operand
        // (N1570 6.5p3). Do not substitute any incoming fact into that tree.
        if has_write(expr) {
            self.boundary();
        }
        self.fold(expr);
    }

    fn initialize(&mut self, local: usize, initializer: &mut ObjectInitializer<'a>) {
        self.values[local] = None;
        initializer_expressions(initializer, |expr| self.expression(expr));
        if self.eligible[local] {
            if let [init] = initializer.values.as_slice() {
                let value = match &init.value {
                    InitValue::Expression(expr) => literal(expr),
                    InitValue::Zero => Some(0),
                };
                self.values[local] = value.map(|bits| canonical(bits, &init.ty, self.target));
            }
        }
    }
}

impl Propagate<'_> {
    fn fold(&self, expr: &mut Expression<'_>) {
        use ExpressionKind::*;
        // These operands designate storage. The value of an integer local
        // cannot replace its lvalue, even when the value is known.
        match &mut expr.kind {
            Unary(Address, _) | Update { .. } | VaStart(_) | VaArg(_) | VaCopy { .. } => {
                return;
            }
            Assignment { right, .. } => self.fold(right),
            _ => children(expr, |child| self.fold(child)),
        }

        if !matches!(expr.ty.kind, Kind::Integer(_)) {
            return;
        }
        let value = match &expr.kind {
            Local(local) => self.values[*local],
            // The translation-time evaluator can convert floating literals.
            // Runtime conversions must preserve live FP exceptions.
            Cast(operand) if !self.preserve_fp || literal(operand).is_some() => {
                constant_integer(expr, self.target)
            }
            Unary(_, operand) if literal(operand).is_some() => constant_integer(expr, self.target),
            Binary {
                op: BitAnd,
                left,
                right,
                ..
            } if (literal(right) == Some(0) && ordinary_local_read(left))
                || (literal(left) == Some(0) && ordinary_local_read(right)) =>
            {
                // N1570 6.5.10. Fold the mask after checking that the
                // dropped local read has no observable effect.
                Some(0)
            }
            Binary {
                op, left, right, ..
            } if literal(left).is_some_and(|value| {
                literal(right).is_some()
                    || (*op == LogicalAnd && value == 0)
                    || (*op == LogicalOr && value != 0)
            }) =>
            {
                constant_integer(expr, self.target)
            }
            Conditional { condition, yes, no } => literal(condition).and_then(|value| {
                let (taken, discarded) = if value == 0 { (no, yes) } else { (yes, no) };
                // N1570 6.5.15p4. Omit a nonconstant arm that cannot execute
                // when the other arm produces an integer literal.
                if literal(discarded).is_none() {
                    literal(taken)?;
                    constant_integer(expr, self.target)
                } else {
                    None
                }
            }),
            _ => None,
        };
        if let Some(value) = value {
            expr.kind = Integer(value);
            expr.ty = expr.ty.unqualified();
        }
    }
}

fn literal(expr: &Expression<'_>) -> Option<u64> {
    match expr.kind {
        ExpressionKind::Integer(bits) => Some(bits),
        _ => None,
    }
}

fn ordinary_local_read(expr: &Expression<'_>) -> bool {
    matches!(expr.kind, ExpressionKind::Local(_))
        && !expr.ty.qualifiers.volatile
        && !expr.ty.qualifiers.atomic
}

fn has_write(expr: &mut Expression<'_>) -> bool {
    if matches!(
        expr.kind,
        ExpressionKind::Assignment { .. }
            | ExpressionKind::Update { .. }
            | ExpressionKind::CompoundLiteral { .. }
            | ExpressionKind::StatementExpression { .. }
    ) {
        return true;
    }
    let mut writes = false;
    children(expr, |child| writes = writes || has_write(child));
    writes
}
