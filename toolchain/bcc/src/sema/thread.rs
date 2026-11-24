//! N1570 §§6.2.2, 6.2.4, 6.7.1, 6.7.9 and 6.9.2.

use super::*;
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct ThreadObject<'a> {
    pub object: Object<'a>,
    pub linkage: Linkage,
    pub initializer: Option<ThreadInitializer<'a>>,
}

pub type ThreadInitializer<'a> = BTreeMap<usize, ThreadData<'a>>;

#[derive(Debug)]
pub enum ThreadData<'a> {
    Byte(u8),
    Address(StaticAddress<'a>, i64),
}

#[derive(Debug)]
pub enum StaticAddress<'a> {
    Symbol(&'a str, &'a Position),
    Local(usize),
    Literal(usize),
    String(Vec<u8>),
}

#[derive(Default)]
pub(super) struct ThreadObjects<'a> {
    pub definitions: Vec<ThreadObject<'a>>,
    indices: HashMap<&'a str, usize>,
}

impl<'a> ThreadObjects<'a> {
    pub(super) fn defines(&self, name: &str) -> bool {
        self.indices.contains_key(name)
    }

    pub(super) fn declare(
        &mut self,
        variable: &mut parse::Variable<'a>,
        linkage: Linkage,
        context: &mut FunctionBody<'a, '_>,
    ) -> Result<bool, Diagnostic> {
        let name = variable.name;
        let key = name.text.as_ref();
        let previous = context.symbols.get(key);
        let explicit = variable.initializer.is_some();
        if explicit && previous.is_some_and(|symbol| symbol.defined) {
            return Err(name.error("_Thread_local object already defined"));
        }
        if let Some(prior_type) = previous
            .map(|symbol| &symbol.ty)
            .or_else(|| context.block_externs.get(key))
        {
            merge_type(name, &mut variable.ty, prior_type)?;
        }
        check_type(name, &variable.ty)?;
        let initializer = variable
            .initializer
            .take()
            .map(|init| context.static_initializer(name, &mut variable.ty, init))
            .transpose()?;
        let ty = &variable.ty;
        let definition = explicit || variable.storage != Storage::Extern;
        if explicit || definition && linkage == Linkage::Internal {
            check_layout(ty, &name.site)?;
        }
        if let Some(&index) = self.indices.get(key) {
            let thread = &mut self.definitions[index];
            thread.object.ty = ty.clone();
            thread.object.alignment = variable
                .alignment
                .filter(|&align| align != 0)
                .or(thread.object.alignment);
            if explicit {
                thread.object.site = &name.site;
                thread.initializer = initializer;
            }
        } else if definition {
            self.indices.insert(key, self.definitions.len());
            self.definitions.push(ThreadObject {
                object: Object {
                    name: key,
                    site: &name.site,
                    ty: ty.clone(),
                    alignment: variable.alignment,
                },
                linkage,
                initializer,
            });
        }
        Ok(explicit || previous.is_some_and(|symbol| symbol.defined))
    }
}

impl<'a> FunctionBody<'a, '_> {
    pub(super) fn thread_local(
        &mut self,
        variable: parse::Variable<'a>,
    ) -> Result<Statement<'a>, Diagnostic> {
        let parse::Variable {
            name,
            mut ty,
            storage,
            initializer,
            alignment,
            ..
        } = variable;
        check_type(name, &ty)?;
        let key = name.text.as_ref();
        let current = self.scopes.last().unwrap().get(key).copied();
        let local_storage = match storage {
            Storage::Static => {
                if current.is_some() {
                    return Err(name.error("duplicate local declaration"));
                }
                if initializer.is_none() {
                    check_layout(&ty, &name.site)?;
                }
                LocalStorage::Thread(None)
            }
            Storage::Extern => {
                if initializer.is_some() {
                    return Err(name.error(
                        "a block-scope extern _Thread_local object cannot have an initializer",
                    ));
                }
                let visible = self.scopes.iter().rev().find_map(|scope| scope.get(key));
                let binding = match visible.map(|&id| &self.locals[id]) {
                    Some(Local {
                        storage: LocalStorage::ThreadExtern(linkage),
                        object,
                    }) => Some((*linkage, &object.ty)),
                    Some(_) => None,
                    None => self
                        .symbols
                        .get(key)
                        .map(|symbol| {
                            if symbol.thread_local {
                                Ok((symbol.linkage, &symbol.ty))
                            } else {
                                Err(name.error(
                                    "_Thread_local must appear in every declaration of the object",
                                ))
                            }
                        })
                        .transpose()?,
                };
                let linkage = if let Some((linkage, prior_type)) = binding {
                    merge_type(name, &mut ty, prior_type)?;
                    linkage
                } else {
                    Linkage::External
                };
                if current.is_some_and(|id| {
                    !matches!(self.locals[id].storage, LocalStorage::ThreadExtern(_))
                }) {
                    return Err(name.error("duplicate local declaration"));
                }
                if linkage == Linkage::External {
                    if let Some(symbol) = self.symbols.get(key) {
                        if !symbol.thread_local || symbol.linkage != Linkage::External {
                            return Err(
                                name.error("conflicting external _Thread_local declaration")
                            );
                        }
                        merge_type(name, &mut ty, &symbol.ty)?;
                    }
                    if let Some(prior_type) = self.block_externs.get(key) {
                        merge_type(name, &mut ty, prior_type)?;
                    }
                    self.block_externs.insert(key, ty.clone());
                }
                LocalStorage::ThreadExtern(linkage)
            }
            _ => {
                return Err(
                    name.error("a block-scope _Thread_local object requires static or extern")
                )
            }
        };
        let id = self.locals.len();
        self.scopes.last_mut().unwrap().insert(key, id);
        self.locals.push(Local {
            object: Object {
                name: key,
                site: &name.site,
                ty: ty.clone(),
                alignment,
            },
            storage: local_storage,
        });
        if let Some(initializer) = initializer {
            let initializer = self.static_initializer(name, &mut ty, initializer)?;
            self.locals[id].object.ty = ty;
            self.locals[id].storage = LocalStorage::Thread(Some(initializer));
        }
        Ok(Statement::Empty)
    }
}

fn check_type(name: &Lexeme, ty: &Type) -> Result<(), Diagnostic> {
    if matches!(ty.kind, Kind::Void | Kind::Function { .. }) {
        return Err(name.error("expected an object type for _Thread_local"));
    }
    Ok(())
}

pub(super) fn check_layout(ty: &Type, site: &Position) -> Result<(), Diagnostic> {
    let size = ty
        .complete_layout()
        .ok_or_else(|| {
            Diagnostic::new(
                site,
                "_Thread_local definition requires a complete object type",
            )
        })?
        .size;
    if size > i32::MAX as usize {
        return Err(Diagnostic::new(
            site,
            "_Thread_local object exceeds the addressable data range",
        ));
    }
    Ok(())
}

fn merge_type(name: &Lexeme, ty: &mut Type, previous: &Type) -> Result<(), Diagnostic> {
    merge_types(ty, previous).ok_or_else(|| name.error("incompatible _Thread_local redeclaration"))
}

pub(super) fn merge_types(ty: &mut Type, previous: &Type) -> Option<()> {
    if ty == previous {
        return Some(());
    }
    if ty.qualifiers == previous.qualifiers {
        if let (Kind::Array(element, count), Kind::Array(prior_element, prior_count)) =
            (&mut ty.kind, &previous.kind)
        {
            if element == prior_element && (count.is_none() || prior_count.is_none()) {
                *count = (*count).max(*prior_count);
                return Some(());
            }
        }
    }
    None
}

pub(super) enum StaticValue<'a> {
    Bits(u128),
    Address(StaticAddress<'a>, i64),
}

pub(super) fn constant_initializer_value<'a>(
    value: &Expression<'a>,
    ty: &Type,
    target: Target,
    locals: &[Local<'a>],
) -> Result<StaticValue<'a>, Diagnostic> {
    let bits = match ty.kind {
        Kind::Integer(integer) => {
            let bits = body::constant_integer(value, target)
                .or_else(|| float::constant_to_integer(value, integer, target));
            bits.map(|bits| {
                if integer == Integer::Bool {
                    u64::from(bits != 0)
                } else {
                    bits
                }
            })
            .map(u128::from)
        }
        Kind::Float(_) | Kind::LongDouble => float::constant_bits(value, &ty.kind, target),
        Kind::Pointer(_) => {
            if let Some((address, addend)) = address_constant(value, target, locals) {
                return Ok(StaticValue::Address(address, addend));
            }
            null_address(value, target).then_some(0)
        }
        _ => None,
    };
    bits.map(StaticValue::Bits).ok_or_else(|| {
        value
            .site
            .error("expected a supported constant in static initializer")
    })
}

pub(super) fn encode_initializer<'a>(
    values: Vec<Initializer<'a>>,
    target: Target,
    locals: &[Local<'a>],
) -> Result<ThreadInitializer<'a>, Diagnostic> {
    let mut data = ThreadInitializer::default();
    for init in values {
        let size = init.ty.layout().unwrap().size;
        let InitValue::Expression(value) = init.value else {
            data.retain(|&offset, _| !(init.offset..init.offset + size).contains(&offset));
            continue;
        };
        let bits = match constant_initializer_value(&value, &init.ty, target, locals)? {
            StaticValue::Bits(bits) => bits,
            StaticValue::Address(address, addend) => {
                data.insert(init.offset, ThreadData::Address(address, addend));
                continue;
            }
        };

        if let Some(field) = init.bit_field {
            for (index, (shift, used, mask)) in field.bytes().enumerate() {
                let ThreadData::Byte(byte) = data
                    .entry(init.offset + index)
                    .or_insert(ThreadData::Byte(0))
                else {
                    unreachable!("bit-fields do not overlap address initializers");
                };
                *byte |= ((bits >> used) as u8 & mask) << shift;
            }
        } else {
            let bytes = bits.to_le_bytes();
            let size = init.ty.layout().unwrap().size;
            data.extend(
                bytes[..size]
                    .iter()
                    .enumerate()
                    .filter_map(|(index, &byte)| {
                        (byte != 0).then_some((init.offset + index, ThreadData::Byte(byte)))
                    }),
            );
        }
    }
    data.retain(|_, value| !matches!(value, ThreadData::Byte(0)));
    Ok(data)
}

pub(super) fn null_address(expr: &Expression<'_>, target: Target) -> bool {
    match (&expr.ty.kind, &expr.kind) {
        (Kind::Pointer(_), ExpressionKind::Cast(operand)) => null_address(operand, target),
        (
            Kind::Pointer(_),
            ExpressionKind::OmittedConditional {
                value, fallback, ..
            },
        ) => {
            let use_fallback = float::constant_truth(value, target)
                .map_or_else(|| null_address(value, target), |truth| !truth);
            use_fallback && null_address(fallback, target)
        }
        _ => body::constant_integer(expr, target) == Some(0),
    }
}

// N1570 6.6p7–9. A thread-storage address is computed at run time.
fn address_constant<'a>(
    expr: &Expression<'a>,
    target: Target,
    locals: &[Local<'a>],
) -> Option<(StaticAddress<'a>, i64)> {
    match &expr.kind {
        ExpressionKind::Unary(Unary::Address, operand) => object_address(operand, target, locals),
        ExpressionKind::Cast(operand) => address_constant(operand, target, locals),
        ExpressionKind::OmittedConditional {
            value, fallback, ..
        } => {
            if let Some(truth) = float::constant_truth(value, target) {
                address_constant(if truth { value } else { fallback }, target, locals)
            } else if let Some(address) = address_constant(value, target, locals) {
                Some(address)
            } else {
                null_address(value, target)
                    .then(|| address_constant(fallback, target, locals))
                    .flatten()
            }
        }
        ExpressionKind::Binary {
            op: op @ (Binary::Add | Binary::Subtract),
            left,
            right,
            left_type,
            ..
        } => {
            let (pointer, integer, ty) = if matches!(left_type.kind, Kind::Pointer(_)) {
                (left, right, left)
            } else if *op == Binary::Add {
                (right, left, right)
            } else {
                return None;
            };
            let ty = ty.value_type();
            let Kind::Pointer(element) = &ty.kind else {
                return None;
            };
            let width = i64::try_from(element.layout()?.size).ok()?;
            let offset = (body::constant_integer(integer, target)? as i64).checked_mul(width)?;
            let (address, addend) = address_constant(pointer, target, locals)?;
            let addend = if *op == Binary::Subtract {
                addend.checked_sub(offset)
            } else {
                addend.checked_add(offset)
            }?;
            Some((address, addend))
        }
        _ if matches!(expr.ty.kind, Kind::Array(..) | Kind::Function { .. }) => {
            object_address(expr, target, locals)
        }
        _ => None,
    }
}

fn object_address<'a>(
    expr: &Expression<'a>,
    target: Target,
    locals: &[Local<'a>],
) -> Option<(StaticAddress<'a>, i64)> {
    match &expr.kind {
        ExpressionKind::Identifier(name) => Some((StaticAddress::Symbol(name, &expr.site.site), 0)),
        ExpressionKind::StaticLiteral(id) => Some((StaticAddress::Literal(*id), 0)),
        ExpressionKind::Local(id) if matches!(locals[*id].storage, LocalStorage::Static(_)) => {
            Some((StaticAddress::Local(*id), 0))
        }
        ExpressionKind::String(bytes) => Some((StaticAddress::String(bytes.clone()), 0)),
        ExpressionKind::Unary(Unary::Dereference, pointer) => {
            address_constant(pointer, target, locals)
        }
        ExpressionKind::Member {
            base,
            offset,
            bit_field: None,
        } => {
            let (address, addend) = object_address(base, target, locals)?;
            Some((address, addend.checked_add(i64::try_from(*offset).ok()?)?))
        }
        _ => None,
    }
}
