//! Remove unreachable statements while preserving alternate entries.

use crate::sema::{ExpressionKind, Statement};

pub(super) fn run(body: &mut Vec<Statement<'_>>) {
    prune_block(body);
}

fn prune_block(body: &mut Vec<Statement<'_>>) -> bool {
    let mut falls_through = true;
    body.retain_mut(|statement| {
        if !falls_through && !has_label(statement) {
            return false;
        }
        falls_through = prune(statement);
        !matches!(statement, Statement::Empty)
    });
    falls_through
}

fn prune(statement: &mut Statement<'_>) -> bool {
    use Statement::*;
    match statement {
        Block(body) => prune_block(body),
        If { condition, yes, no } => {
            if let ExpressionKind::Integer(value) = condition.kind {
                let selected = if value == 0 {
                    (!has_label(yes)).then(|| no.take().map_or(Empty, |arm| *arm))
                } else {
                    (!no.as_deref().is_some_and(has_label))
                        .then(|| std::mem::replace(yes.as_mut(), Empty))
                };
                if let Some(selected) = selected {
                    *statement = selected;
                    return prune(statement);
                }
            }
            let yes_falls_through = prune(yes);
            let no_falls_through = no.as_mut().is_none_or(|no| prune(no));
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
                prune(body);
            }
            true
        }
        Switch { body, .. } => {
            prune(body);
            true
        }
        Label(_, body) => prune(body),
        Return(_) | Jump(_) => false,
        Empty | Expression(_) | Initialize { .. } => true,
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
        Empty | Expression(_) | Initialize { .. } | Return(_) | Jump(_) => false,
    }
}
