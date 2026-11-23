//! Reuse integer expressions within a basic block.

use std::collections::HashMap;
use std::ptr::from_ref;

use super::{children, initializer_expressions, walk_blocks, BlockPass};
use crate::sema::{
    Binary, Expression, ExpressionKind, InitValue, Local, LocalStorage, Object, ObjectInitializer,
    Statement, Unary,
};
use crate::types::{Integer, Kind};

pub(super) fn run<'a>(body: &mut [Statement<'a>], locals: &mut Vec<Local<'a>>, eligible: &[bool]) {
    walk_blocks(
        body,
        &mut Eliminate {
            locals,
            eligible,
            values: HashMap::new(),
            available: HashMap::new(),
        },
    );
}

type Value = usize;
type Nodes<'a> = HashMap<*const Expression<'a>, Value>;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Key {
    ty: Integer,
    form: Form,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum Form {
    Literal(u64),
    Local(usize),
    Cast(Value),
    Unary(Unary, Value),
    Binary(Binary, Value, Value, Integer, Integer),
}

struct Eliminate<'f, 'a> {
    locals: &'f mut Vec<Local<'a>>,
    eligible: &'f [bool],
    values: HashMap<Key, Value>,
    available: HashMap<Value, usize>,
}

impl<'a> BlockPass<'a> for Eliminate<'_, 'a> {
    fn boundary(&mut self) {
        self.values.clear();
        self.available.clear();
    }

    fn expression(&mut self, expr: &mut Expression<'a>) {
        if self.rewrite(expr).is_none() {
            self.boundary();
        }
    }

    fn initialize(&mut self, local: usize, initializer: &mut ObjectInitializer<'a>) {
        if self.eligible[local] {
            if let [init] = initializer.values.as_mut_slice() {
                if let InitValue::Expression(expr) = &mut init.value {
                    let same_type = init.ty.kind == expr.ty.kind;
                    let value = self.rewrite(expr);
                    if let Some(value) = value {
                        // A narrower store (or _Bool conversion) cannot hold
                        // the original computation's value in general.
                        if same_type {
                            self.available.insert(value, local);
                        }
                        return;
                    }
                }
            }
        } else {
            initializer_expressions(initializer, |expr| self.expression(expr));
        }
        self.boundary();
    }
}

impl<'a> Eliminate<'_, 'a> {
    fn rewrite(&mut self, expr: &mut Expression<'a>) -> Option<Value> {
        let mut nodes = HashMap::new();
        let root = self.identify(expr, &mut nodes)?;

        let mut uses = HashMap::new();
        count_uses(expr, &nodes, &self.available, &mut uses);
        let mut saved = Vec::new();
        let mut temps = HashMap::new();
        self.replace(expr, &nodes, &uses, &mut temps, &mut saved);
        // N1570 6.5.17. Each private temporary is initialized before
        // any operand reads it, including operands with unspecified order.
        for assignment in saved.into_iter().rev() {
            let value = Expression {
                site: expr.site,
                ty: expr.ty.clone(),
                kind: std::mem::replace(&mut expr.kind, ExpressionKind::Integer(0)),
            };
            expr.kind = ExpressionKind::Comma(Box::new(assignment), Box::new(value));
        }
        Some(root)
    }

    fn identify(&mut self, expr: &Expression<'a>, nodes: &mut Nodes<'a>) -> Option<Value> {
        use ExpressionKind::*;
        let ty = integer(&expr.ty.kind)?;
        if expr.ty.qualifiers.volatile || expr.ty.qualifiers.atomic {
            return None;
        }
        let form = match &expr.kind {
            Local(local) if self.eligible.get(*local) == Some(&true) => Form::Local(*local),
            Integer(bits) => Form::Literal(*bits),
            Cast(operand) => Form::Cast(self.identify(operand, nodes)?),
            Unary(op, operand)
                if !matches!(
                    op,
                    crate::sema::Unary::Address | crate::sema::Unary::Dereference
                ) =>
            {
                Form::Unary(*op, self.identify(operand, nodes)?)
            }
            Binary {
                op,
                left,
                right,
                left_type,
                right_type,
            } if !matches!(
                op,
                crate::sema::Binary::LogicalAnd | crate::sema::Binary::LogicalOr
            ) =>
            {
                Form::Binary(
                    *op,
                    self.identify(left, nodes)?,
                    self.identify(right, nodes)?,
                    integer(&left_type.kind)?,
                    integer(&right_type.kind)?,
                )
            }
            // The work moved to the enclosing comma must have executed
            // unconditionally and have no observable reads or writes.
            _ => return None,
        };
        let next = self.values.len();
        let id = *self.values.entry(Key { ty, form }).or_insert(next);
        nodes.insert(from_ref(expr), id);
        Some(id)
    }

    fn replace(
        &mut self,
        expr: &mut Expression<'a>,
        nodes: &Nodes<'a>,
        uses: &HashMap<Value, usize>,
        temps: &mut HashMap<Value, usize>,
        saved: &mut Vec<Expression<'a>>,
    ) {
        let id = nodes[&from_ref(expr)];
        // Reusing an integer literal or a direct local read cannot save
        // an evaluation. Keep the emitted literal or original local.
        if matches!(
            expr.kind,
            ExpressionKind::Integer(_) | ExpressionKind::Local(_)
        ) {
            return;
        }
        if let Some(local) = self.available.get(&id).or_else(|| temps.get(&id)) {
            expr.kind = ExpressionKind::Local(*local);
            return;
        }

        children(expr, |child| self.replace(child, nodes, uses, temps, saved));
        if uses[&id] > 1 {
            let local = self.locals.len();
            self.locals.push(Local {
                object: Object {
                    name: "",
                    site: &expr.site.site,
                    ty: expr.ty.clone(),
                    alignment: None,
                },
                storage: LocalStorage::Automatic,
            });
            let read = Expression {
                site: expr.site,
                ty: expr.ty.clone(),
                kind: ExpressionKind::Local(local),
            };
            let value = std::mem::replace(expr, read);
            saved.push(Expression {
                site: expr.site,
                ty: expr.ty.clone(),
                kind: ExpressionKind::Assignment {
                    left: Box::new(Expression {
                        site: expr.site,
                        ty: expr.ty.clone(),
                        kind: ExpressionKind::Local(local),
                    }),
                    right: Box::new(value),
                    operation: None,
                },
            });
            temps.insert(id, local);
        }
    }
}

fn integer(kind: &Kind) -> Option<Integer> {
    match kind {
        Kind::Integer(integer) => Some(*integer),
        _ => None,
    }
}

fn count_uses<'a>(
    expr: &mut Expression<'a>,
    nodes: &Nodes<'a>,
    available: &HashMap<Value, usize>,
    uses: &mut HashMap<Value, usize>,
) {
    let id = nodes[&from_ref(expr)];
    let count = uses.entry(id).or_default();
    *count += 1;
    // Once a parent is shared, its children will be evaluated only once
    // for that parent, even if it appeared more than once in the AST.
    if *count == 1 && !available.contains_key(&id) {
        children(expr, |child| count_uses(child, nodes, available, uses));
    }
}
