//! Remove unreachable statements while preserving alternate entries.

use std::collections::HashMap;

use super::{children, initializer_expressions};
use crate::sema::{Expression, ExpressionKind, Statement};

pub(super) fn run(body: &mut Vec<Statement<'_>>) {
    let mut entries = Entries::default();
    for statement in body.iter_mut() {
        entries.scan(statement);
    }
    prune_block(body, &entries);
}

fn prune_block(body: &mut Vec<Statement<'_>>, entries: &Entries) -> bool {
    let mut falls_through = true;
    body.retain_mut(|statement| {
        if !falls_through && !has_label(statement) {
            return false;
        }
        falls_through = prune(statement, entries);
        !matches!(statement, Statement::Empty)
    });
    falls_through
}

fn prune(statement: &mut Statement<'_>, entries: &Entries) -> bool {
    use Statement::*;
    match statement {
        Block(body) => prune_block(body, entries),
        If { condition, yes, no } => {
            if let ExpressionKind::Integer(value) = condition.kind {
                let selected = if value == 0 {
                    (!entries.enters_from_outside(yes))
                        .then(|| no.take().map_or(Empty, |arm| *arm))
                } else {
                    (!no.as_deref_mut().is_some_and(|arm| entries.enters_from_outside(arm)))
                        .then(|| std::mem::replace(yes.as_mut(), Empty))
                };
                if let Some(selected) = selected {
                    *statement = selected;
                    return prune(statement, entries);
                }
            }
            let yes_falls_through = prune(yes, entries);
            let no_falls_through = no.as_mut().is_none_or(|no| prune(no, entries));
            yes_falls_through || no_falls_through
        }
        Loop {
            condition,
            post_test,
            body,
            ..
        } => {
            if !*post_test
                && condition
                    .as_ref()
                    .is_some_and(|expr| matches!(expr.kind, ExpressionKind::Integer(0)))
                && !has_label(body)
            {
                *statement = Empty;
            } else {
                prune(body, entries);
            }
            true
        }
        Switch { body, .. } => {
            prune(body, entries);
            true
        }
        Label(_, body) => prune(body, entries),
        Return(_) | Jump(..) => false,
        Empty | InlineAsm(_) | Expression(_) | Initialize { .. } | SaveVlaStack(_) | RestoreVlaStack(_) => true,
    }
}

#[derive(Default)]
struct Entries {
    labels: Vec<usize>,
    references: HashMap<usize, usize>,
}

impl Entries {
    fn enters_from_outside(&self, arm: &mut Statement<'_>) -> bool {
        let mut local = Self::default();
        local.scan(arm);
        local.labels.iter().any(|label| {
            self.references.get(label).copied().unwrap_or(0)
                > local.references.get(label).copied().unwrap_or(0)
        })
    }

    fn scan(&mut self, statement: &mut Statement<'_>) {
        use Statement::*;
        match statement {
            Label(id, body) => {
                self.labels.push(*id);
                self.scan(body);
            }
            Jump(id, _) => *self.references.entry(*id).or_default() += 1,
            Switch { value, cases, body, .. } => {
                for (_, id) in cases {
                    *self.references.entry(*id).or_default() += 1;
                }
                self.expression(value);
                self.scan(body);
            }
            Block(body) => {
                for statement in body {
                    self.scan(statement);
                }
            }
            If { condition, yes, no } => {
                self.expression(condition);
                self.scan(yes);
                if let Some(no) = no {
                    self.scan(no);
                }
            }
            Loop { condition, increment, body, .. } => {
                for expr in condition.iter_mut().chain(increment.iter_mut()) {
                    self.expression(expr);
                }
                self.scan(body);
            }
            Initialize { initializer, .. } => {
                initializer_expressions(initializer, |expr| self.expression(expr));
            }
            Expression(expr) | Return(Some(expr)) => self.expression(expr),
            InlineAsm(asm) => {
                for register in &mut asm.registers {
                    self.expression(&mut register.slot);
                }
            }
            Empty | Return(None) | SaveVlaStack(_) | RestoreVlaStack(_) => {}
        }
    }

    fn expression(&mut self, expr: &mut Expression<'_>) {
        // A GNU statement expression can jump to a label outside the
        // expression. Keep those references in the containing function.
        if let ExpressionKind::StatementExpression { statements, value, .. } = &mut expr.kind {
            for statement in statements {
                self.scan(statement);
            }
            if let Some(value) = value {
                self.expression(value);
            }
        } else {
            children(expr, |child| self.expression(child));
        }
    }
}

// N1570 6.8.4.2 and 6.8.6.1. Case labels and goto targets can be
// reached even when the containing statement has no fall-through entry.
fn has_label(statement: &Statement<'_>) -> bool {
    use Statement::*;
    match statement {
        Label(..) => true,
        Block(body) => body.iter().any(has_label),
        If { yes, no, .. } => has_label(yes) || no.as_deref().is_some_and(has_label),
        Loop { body, .. } | Switch { body, .. } => has_label(body),
        Empty | InlineAsm(_) | Expression(_) | Initialize { .. } | SaveVlaStack(_) | RestoreVlaStack(_) | Return(_) | Jump(..) => false,
    }
}
