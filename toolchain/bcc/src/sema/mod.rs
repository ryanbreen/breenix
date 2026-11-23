//! Typed translation units for the hello subset of N1570.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::parse::{self, Storage};
use crate::pp::{self, Diagnostic, Lexeme, Position, Preprocessed, Target};
use crate::types::{BitField, Float, Integer, Kind, Record, Type};
pub use parse::{Binary, Unary};

mod atomic;
mod body;
mod c11;
mod dump;
mod float;
mod thread;
mod varargs;

pub(super) use c11::{check_static_assertion, type_alignment};

pub use atomic::AtomicOp;
pub(crate) use atomic::SEQ_CST;
pub use thread::{StaticAddress, ThreadData, ThreadInitializer, ThreadObject};

#[derive(Debug)]
pub struct TranslationUnit<'a> {
    pub functions: Vec<Function<'a>>,
    pub objects: Vec<Object<'a>>,
    pub static_objects: Vec<StaticObject<'a>>,
    pub static_literals: Vec<StaticLiteral<'a>>,
    pub thread_objects: Vec<ThreadObject<'a>>,
    pub records: Vec<Record<'a>>,
    deferred_atomic_type: Option<&'a Lexeme>,
}

impl TranslationUnit<'_> {
    pub(crate) fn check_target_types(&self) -> Result<(), Diagnostic> {
        match self.deferred_atomic_type {
            Some(token) => Err(token.error("atomic types are deferred to ARM 25.9d")),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Linkage {
    Internal,
    External,
}

#[derive(Debug)]
pub struct Function<'a> {
    pub name: &'a str,
    pub site: &'a Position,
    pub ty: Type,
    pub linkage: Linkage,
    pub params: Vec<Parameter<'a>>,
    pub noreturn: bool,
    pub body: Option<Vec<Statement<'a>>>,
    pub locals: Vec<Local<'a>>,
}

#[derive(Debug)]
pub struct Parameter<'a> {
    pub name: Option<(&'a str, &'a Position)>,
    pub ty: Type,
}

#[derive(Debug)]
pub struct Object<'a> {
    pub name: &'a str,
    pub site: &'a Position,
    pub ty: Type,
    pub alignment: Option<usize>,
}

impl Object<'_> {
    pub(crate) fn layout(&self) -> Option<crate::types::Layout> {
        let mut layout = self.ty.layout()?;
        layout.align = layout.align.max(self.alignment.unwrap_or(0));
        Some(layout)
    }
}

#[derive(Debug)]
pub struct StaticObject<'a> {
    pub object: Object<'a>,
    pub linkage: Linkage,
    pub initializer: Option<ThreadInitializer<'a>>,
}

#[derive(Debug)]
pub struct StaticLiteral<'a> {
    pub ty: Type,
    pub site: &'a Position,
    pub initializer: ThreadInitializer<'a>,
}

#[derive(Debug)]
pub struct Local<'a> {
    pub object: Object<'a>,
    pub storage: LocalStorage<'a>,
}

#[derive(Debug)]
pub enum LocalStorage<'a> {
    Automatic,
    Static(Option<ThreadInitializer<'a>>),
    Function(Linkage),
    Thread(Option<ThreadInitializer<'a>>),
    ThreadExtern(Linkage),
}

#[derive(Debug)]
pub struct Initializer<'a> {
    pub offset: usize,
    pub bit_field: Option<BitField>,
    pub value: InitValue<'a>,
    pub ty: Type,
}

#[derive(Debug)]
pub enum InitValue<'a> {
    Expression(Expression<'a>),
    Zero,
}

#[derive(Debug)]
pub struct ObjectInitializer<'a> {
    pub values: Vec<Initializer<'a>>,
    pub zero_bytes: usize,
}

#[derive(Debug)]
pub enum Statement<'a> {
    Block(Vec<Self>),
    Empty,
    Initialize {
        local: usize,
        initializer: ObjectInitializer<'a>,
    },
    Expression(Expression<'a>),
    Return(Option<Expression<'a>>),
    If {
        condition: Expression<'a>,
        yes: Box<Self>,
        no: Option<Box<Self>>,
    },
    Loop {
        condition: Option<Expression<'a>>,
        post_test: bool,
        increment: Option<Expression<'a>>,
        body: Box<Self>,
        break_to: usize,
        continue_to: usize,
    },
    Switch {
        value: Expression<'a>,
        ty: Type,
        cases: Vec<(Option<u64>, usize)>,
        body: Box<Self>,
        break_to: usize,
    },
    Label(usize, Box<Self>),
    Jump(usize),
}

#[derive(Debug)]
pub struct Expression<'a> {
    site: &'a Lexeme,
    pub kind: ExpressionKind<'a>,
    pub ty: Type,
}

#[derive(Debug)]
pub enum ExpressionKind<'a> {
    Identifier(&'a str),
    ThreadLocal(&'a str),
    Local(usize),
    CompoundLiteral {
        local: usize,
        initializer: ObjectInitializer<'a>,
    },
    StaticLiteral(usize),
    Integer(u64),
    Float(u128),
    Unary(Unary, Box<Expression<'a>>),
    Binary {
        op: Binary,
        left: Box<Expression<'a>>,
        right: Box<Expression<'a>>,
        left_type: Type,
        right_type: Type,
    },
    Comma(Box<Expression<'a>>, Box<Expression<'a>>),
    Cast(Box<Expression<'a>>),
    Conditional {
        condition: Box<Expression<'a>>,
        yes: Box<Expression<'a>>,
        no: Box<Expression<'a>>,
    },
    Assignment {
        left: Box<Expression<'a>>,
        right: Box<Expression<'a>>,
        operation: Option<(Binary, Type, Type)>,
    },
    Update {
        operand: Box<Expression<'a>>,
        increment: bool,
        postfix: bool,
    },
    Member {
        base: Box<Expression<'a>>,
        offset: usize,
        bit_field: Option<BitField>,
    },
    String(Vec<u8>),
    VaStart(Box<Expression<'a>>),
    VaArg(Box<Expression<'a>>),
    VaCopy {
        dest: Box<Expression<'a>>,
        source: Box<Expression<'a>>,
    },
    VaEnd(Box<Expression<'a>>),
    FltRounds,
    Atomic {
        op: AtomicOp,
        arguments: Vec<Argument<'a>>,
    },
    Call {
        callee: Box<Expression<'a>>,
        arguments: Vec<Argument<'a>>,
    },
}

#[derive(Debug)]
pub struct Argument<'a> {
    pub expr: Expression<'a>,
    /// Type passed after assignment conversion or default argument promotion.
    pub ty: Type,
}

impl Expression<'_> {
    pub fn position(&self) -> &Position {
        &self.site.site
    }

    // N1570 6.3.2.1: value contexts remove the lvalue's top-level
    // qualifiers and convert arrays and functions to pointers.
    pub fn converted_type(&self) -> Option<Type> {
        match &self.ty.kind {
            Kind::Array(element, _) => Some(element.as_ref().clone().pointer_to()),
            Kind::Function { .. } => Some(self.ty.clone().pointer_to()),
            _ if self.lvalue() && self.ty.qualifiers != Default::default() => {
                Some(self.ty.unqualified())
            }
            _ => None,
        }
    }

    pub fn bit_field(&self) -> Option<BitField> {
        match &self.kind {
            ExpressionKind::Member { bit_field, .. } => *bit_field,
            _ => None,
        }
    }

    // N1570 §6.3.1.1p2: an int represents the range of an unsigned
    // int bit-field narrower than 32 bits.
    fn promoted_integer(&self, integer: Integer) -> Integer {
        let field = match &self.kind {
            ExpressionKind::Assignment { left, .. } => left.bit_field(),
            ExpressionKind::Update {
                operand,
                postfix: false,
                ..
            } => operand.bit_field(),
            _ => self.bit_field(),
        };
        if integer == Integer::UnsignedInt && field.is_some_and(|field| field.width < 32) {
            Integer::Int
        } else {
            integer.promoted()
        }
    }

    fn lvalue(&self) -> bool {
        if matches!(self.ty.kind, Kind::Function { .. }) {
            return false;
        }
        match &self.kind {
            ExpressionKind::Identifier(_)
            | ExpressionKind::ThreadLocal(_)
            | ExpressionKind::Local(_)
            | ExpressionKind::CompoundLiteral { .. }
            | ExpressionKind::StaticLiteral(_)
            | ExpressionKind::String(_)
            | ExpressionKind::Unary(Unary::Dereference, _) => true,
            ExpressionKind::Member { base, .. } => base.lvalue(),
            _ => false,
        }
    }

    fn modifiable(&self, records: &[Record<'_>]) -> bool {
        self.lvalue()
            && !self.ty.contains_const(records)
            && (is_scalar(&self.ty)
                || matches!(self.ty.kind, Kind::ShortVector { .. })
                || matches!(&self.ty.kind, Kind::Record(record) if record.layout.is_some()))
    }

    fn value_type(&self) -> Type {
        self.converted_type().unwrap_or_else(|| self.ty.clone())
    }
}

pub fn analyze<'a>(
    source: &'a Preprocessed,
    path: &Path,
    target: Target,
) -> Result<TranslationUnit<'a>, Diagnostic> {
    let mut symbols = HashMap::new();
    let mut functions = Vec::new();
    let mut objects = Vec::new();
    let mut static_objects = Vec::new();
    let mut tentative = Vec::new();
    let mut static_literals = Vec::new();
    let mut thread_objects = thread::ThreadObjects::default();
    let mut block_externs = HashMap::new();
    let mut declarations = parse::translation_unit(source, path, target)?;
    let records_begin = declarations
        .iter()
        .position(|declaration| matches!(declaration, parse::Declaration::Record(_)))
        .unwrap_or(declarations.len());
    let records: Vec<_> = declarations
        .drain(records_begin..)
        .map(|decl| {
            let parse::Declaration::Record(record) = decl else {
                unreachable!()
            };
            record
        })
        .collect();
    let token_order = if records.is_empty() {
        HashMap::new()
    } else {
        source
            .tokens()
            .enumerate()
            .map(|(index, token)| (std::ptr::from_ref(token), index))
            .collect()
    };
    let record_scope = RecordScope {
        records: &records,
        token_order: Some(&token_order),
    };
    for declaration in declarations {
        match declaration {
            parse::Declaration::Record(_) => unreachable!(),
            parse::Declaration::StaticAssert(assertion) => {
                let result = Type::new(Kind::Void);
                let mut context = FunctionBody::new(
                    &symbols,
                    &result,
                    target,
                    &[],
                    &record_scope,
                    &mut block_externs,
                );
                context.static_assertion(assertion)?;
            }
            parse::Declaration::Function(declaration) => {
                functions.push(function(
                    declaration,
                    &mut symbols,
                    target,
                    &record_scope,
                    &mut block_externs,
                )?);
            }
            parse::Declaration::Object(mut variable) => {
                let name = variable.name;
                let key = name.text.as_ref();
                let previous = symbols.get(key);
                let explicit = variable.initializer.is_some();
                let definition = explicit || variable.storage != Storage::Extern;
                let alignment = variable.alignment.filter(|&align| align != 0);
                if let Some(prior) =
                    previous.filter(|prior| !matches!(prior.ty.kind, Kind::Function { .. }))
                {
                    if alignment.is_some()
                        && prior.alignment.is_some()
                        && alignment != prior.alignment
                        || explicit && prior.alignment.is_some() && alignment.is_none()
                        || prior.defined && prior.alignment.is_none() && alignment.is_some()
                    {
                        return Err(name.error("conflicting object alignment declarations"));
                    }
                }
                if previous.is_some_and(|symbol| symbol.thread_local != variable.thread_local)
                    || !variable.thread_local && block_externs.contains_key(key)
                {
                    return Err(
                        name.error("_Thread_local must appear in every declaration of the object")
                    );
                }
                if variable.thread_local {
                    let linkage = match variable.storage {
                        Storage::Static => Linkage::Internal,
                        Storage::Extern => {
                            previous.map_or(Linkage::External, |symbol| symbol.linkage)
                        }
                        _ => Linkage::External,
                    };
                    if previous.is_some_and(|symbol| symbol.linkage != linkage)
                        || linkage == Linkage::Internal && block_externs.contains_key(key)
                    {
                        return Err(name.error("conflicting linkage for _Thread_local object"));
                    }
                    let result = Type::new(Kind::Void);
                    let mut context = FunctionBody::new(
                        &symbols,
                        &result,
                        target,
                        &[],
                        &record_scope,
                        &mut block_externs,
                    );
                    context.static_literals = Some((static_literals.len(), Vec::new()));
                    let defined = thread_objects.declare(&mut variable, linkage, &mut context)?;
                    static_literals.extend(context.static_literals.take().unwrap().1);
                    symbols.insert(
                        key,
                        Symbol {
                            ty: variable.ty.clone(),
                            linkage,
                            defined,
                            thread_local: true,
                            alignment: alignment.or(previous.and_then(|prior| prior.alignment)),
                            noreturn: false,
                        },
                    );
                    if thread_objects.defines(key) {
                        continue;
                    }
                } else {
                    if variable.ty.kind == Kind::Void
                        || matches!(variable.ty.kind, Kind::Function { .. })
                    {
                        return Err(name.error("invalid object type"));
                    }
                    let linkage = match (variable.storage, previous) {
                        (Storage::Static, _) => Linkage::Internal,
                        (_, Some(symbol)) => symbol.linkage,
                        (_, None) => Linkage::External,
                    };
                    if let Some(symbol) = previous {
                        if symbol.linkage != linkage
                            || matches!(symbol.ty.kind, Kind::Function { .. })
                        {
                            return Err(name.error("incompatible redeclaration"));
                        }
                        thread::merge_types(&mut variable.ty, &symbol.ty)
                            .ok_or_else(|| name.error("incompatible redeclaration"))?;
                        if explicit && symbol.defined {
                            return Err(name.error("object already defined"));
                        }
                    }
                    if !explicit && definition {
                        tentative.push((key, name));
                    }
                    symbols.insert(
                        key,
                        Symbol {
                            ty: variable.ty.clone(),
                            linkage,
                            thread_local: false,
                            defined: explicit || previous.is_some_and(|symbol| symbol.defined),
                            alignment: alignment.or(previous.and_then(|prior| prior.alignment)),
                            noreturn: false,
                        },
                    );
                    if let Some(initializer) = variable.initializer {
                        let result = Type::new(Kind::Void);
                        let mut context = FunctionBody::new(
                            &symbols,
                            &result,
                            target,
                            &[],
                            &record_scope,
                            &mut block_externs,
                        );
                        context.static_literals = Some((static_literals.len(), Vec::new()));
                        let initializer =
                            context.static_initializer(name, &mut variable.ty, initializer)?;
                        static_literals.extend(context.static_literals.take().unwrap().1);
                        symbols.get_mut(key).unwrap().ty = variable.ty.clone();
                        static_objects.push(StaticObject {
                            object: Object {
                                name: key,
                                site: &name.site,
                                ty: variable.ty,
                                alignment,
                            },
                            linkage,
                            initializer: Some(initializer),
                        });
                        continue;
                    }
                }
                objects.push(Object {
                    name: &name.text,
                    site: &name.site,
                    ty: variable.ty,
                    alignment,
                });
            }
        }
    }
    for (key, site) in tentative {
        let symbol = symbols.get_mut(key).unwrap();
        if symbol.defined {
            continue;
        }
        let mut ty = symbol.ty.clone();
        if let Kind::Array(_, count @ 0) = &mut ty.kind {
            *count = 1;
        }
        ty.resolve_records(&|id| records[id].definition.as_ref().map(|d| d.layout));
        thread::check_layout(&ty, &site.site)?;
        let alignment = symbol.alignment;
        symbol.defined = true;
        static_objects.push(StaticObject {
            object: Object {
                name: key,
                site: &site.site,
                ty,
                alignment,
            },
            linkage: symbol.linkage,
            initializer: None,
        });
    }
    for function in &mut functions {
        function.noreturn = symbols[function.name].noreturn;
    }
    for thread in &mut thread_objects.definitions {
        // N1570 6.9.2: a remaining external tentative array with no bound
        // is completed as a one-element array at the end of the unit.
        if let Kind::Array(_, count @ 0) = &mut thread.object.ty.kind {
            *count = 1;
        }
        thread
            .object
            .ty
            .resolve_records(&|id| records[id].definition.as_ref().map(|d| d.layout));
        thread::check_layout(&thread.object.ty, thread.object.site)?;
    }
    // N1570 6.7.2.4p2 and 6.10.8.3: the ARM profile lacks atomic
    // types. Preserve ordinary constraint diagnostics and let the backend
    // name the access site before falling back to the keyword location.
    let deferred_atomic_type = if target == Target::Aarch64 {
        source.tokens().find(|token| token.text.as_ref() == "_Atomic")
    } else {
        None
    };
    Ok(TranslationUnit {
        functions,
        objects,
        static_objects,
        static_literals,
        thread_objects: thread_objects.definitions,
        records,
        deferred_atomic_type,
    })
}

struct RecordScope<'a, 'r> {
    records: &'r [Record<'a>],
    token_order: Option<&'r HashMap<*const Lexeme, usize>>,
}

impl RecordScope<'_, '_> {
    // A tag becomes complete at its closing brace, even when analysis runs
    // after parsing. Token order also works across macro expansions/includes.
    fn resolve(&self, ty: &mut Type, site: &Lexeme) {
        ty.resolve_records(&|id| {
            let def = self.records.get(id)?.definition.as_ref()?;
            if let (Some(order), Some(end)) = (self.token_order, def.end) {
                let end = order[&std::ptr::from_ref(end)];
                let use_at = order[&std::ptr::from_ref(site)];
                if end >= use_at {
                    return None;
                }
            }
            Some(def.layout)
        });
    }
}

struct Symbol {
    ty: Type,
    linkage: Linkage,
    defined: bool,
    thread_local: bool,
    alignment: Option<usize>,
    noreturn: bool,
}

/// Evaluate a layout constant while parsing a record. The parser's record
/// table already reflects which tags are complete at this point.
pub(super) fn layout_constant<'a, 't>(
    expression: parse::Expression<'a>,
    target: Target,
    records: &[Record<'a>],
    names: impl Iterator<Item = (&'a str, Option<&'t Type>)>,
) -> Result<u64, Diagnostic> {
    let mut symbols = HashMap::new();
    for (name, ty) in names {
        if let Some(ty) = ty {
            symbols.insert(
                name,
                Symbol {
                    ty: ty.clone(),
                    linkage: Linkage::External,
                    defined: false,
                    thread_local: false,
                    alignment: None,
                    noreturn: false,
                },
            );
        } else {
            symbols.remove(name);
        }
    }
    let record_scope = RecordScope {
        records,
        token_order: None,
    };
    let result = Type::new(Kind::Void);
    let mut block_externs = HashMap::new();
    let mut context = FunctionBody::new(
        &symbols,
        &result,
        target,
        &[],
        &record_scope,
        &mut block_externs,
    );
    let expr = context.expression(expression)?;
    body::constant_integer(&expr, target)
        .ok_or_else(|| expr.site.error("expected an integer constant expression"))
}

fn function<'a>(
    declaration: parse::Function<'a>,
    symbols: &mut HashMap<&'a str, Symbol>,
    target: Target,
    records: &RecordScope<'a, '_>,
    block_externs: &mut HashMap<&'a str, Type>,
) -> Result<Function<'a>, Diagnostic> {
    let parse::Function {
        name,
        result,
        params,
        variadic,
        prototype,
        noreturn,
        storage,
        body,
    } = declaration;
    let defined = body.is_some();
    if defined && matches!(result.kind, Kind::Record(record) if record.layout.is_none()) {
        return Err(name.error("function definition has an incomplete record result"));
    }
    let mut names = HashSet::new();
    let params: Vec<_> = params
        .into_iter()
        .map(|param| {
            if defined && matches!(param.ty.kind, Kind::Record(record) if record.layout.is_none()) {
                return Err(param.start.error("parameter has an incomplete record type"));
            }
            if let Some(token) = param.name {
                if !names.insert(token.text.as_ref()) {
                    return Err(token.error("duplicate parameter name"));
                }
            } else if defined {
                return Err(param
                    .start
                    .error("parameter needs a name in a function definition"));
            }
            Ok(Parameter {
                name: param.name.map(|token| (token.text.as_ref(), &token.site)),
                ty: param.ty,
            })
        })
        .collect::<Result<_, _>>()?;
    let ty = Type::new(Kind::Function {
        result: Box::new(result.clone()),
        // N1570 6.7.6.3p15: qualifiers on the parameter itself do not
        // participate in function type compatibility.
        params: params
            .iter()
            .map(|param| {
                if prototype {
                    param.ty.unqualified()
                } else {
                    param.ty.default_argument_type()
                }
            })
            .collect(),
        variadic,
        prototype,
    });
    let previous = symbols.get(name.text.as_ref());
    if previous.is_some_and(|symbol| symbol.thread_local)
        || block_externs.contains_key(name.text.as_ref())
    {
        return Err(name.error("a function cannot redeclare a _Thread_local object"));
    }
    // N1570 6.2.2p3–p5: a declaration with extern linkage rules inherits
    // the linkage of a visible declaration.
    let linkage = match (storage, previous) {
        (Storage::Static, _) => Linkage::Internal,
        (_, Some(symbol)) => symbol.linkage,
        (_, None) => Linkage::External,
    };
    if let Some(symbol) = previous {
        if !compatible_function_declaration(&ty, defined, &symbol.ty, symbol.defined)
            || symbol.linkage != linkage
        {
            return Err(name.error("incompatible redeclaration"));
        }
        if symbol.defined && defined {
            return Err(name.error("function already defined"));
        }
    }
    let visible_type = previous
        .filter(|symbol| !prototype && (has_prototype(&symbol.ty) || symbol.defined && !defined))
        .map_or(&ty, |symbol| &symbol.ty)
        .clone();
    let noreturn = noreturn || previous.is_some_and(|symbol| symbol.noreturn);
    symbols.insert(
        &name.text,
        Symbol {
            ty: visible_type,
            linkage,
            defined: defined || previous.is_some_and(|s| s.defined),
            thread_local: false,
            alignment: None,
            noreturn,
        },
    );
    let (body, locals) = if let Some(statements) = body {
        let mut context =
            FunctionBody::new(symbols, &result, target, &params, records, block_externs);
        if variadic {
            context.last_named_parameter = params.len().checked_sub(1);
        }
        let body = context.statements(statements)?;
        context.validate_labels()?;
        (Some(body), context.locals)
    } else {
        (None, Vec::new())
    };
    Ok(Function {
        name: &name.text,
        site: &name.site,
        ty,
        linkage,
        params,
        noreturn,
        body,
        locals,
    })
}

fn has_prototype(ty: &Type) -> bool {
    matches!(
        ty.kind,
        Kind::Function {
            prototype: true,
            ..
        }
    )
}

fn compatible_function_declaration(a: &Type, a_defined: bool, b: &Type, b_defined: bool) -> bool {
    let (
        Kind::Function {
            params: a_params, ..
        },
        Kind::Function {
            params: b_params, ..
        },
    ) = (&a.kind, &b.kind)
    else {
        return false;
    };
    a.compatible(b)
        && (!(a_defined && !has_prototype(a) && has_prototype(b)
            || b_defined && !has_prototype(b) && has_prototype(a))
            || a_params.len() == b_params.len())
}

struct FunctionBody<'a, 's> {
    symbols: &'s HashMap<&'a str, Symbol>,
    // Declarations with external linkage must agree even when they occur
    // in blocks that are outside each other's lexical scope.
    block_externs: &'s mut HashMap<&'a str, Type>,
    locals: Vec<Local<'a>>,
    // Present only while analyzing file-scope initializers.
    static_literals: Option<(usize, Vec<StaticLiteral<'a>>)>,
    scopes: Vec<HashMap<&'a str, usize>>,
    labels: HashMap<&'a str, body::Label<'a>>,
    next_label: usize,
    exit: Option<usize>,
    repeat: Option<usize>,
    switches: Vec<body::Switch>,
    result: &'s Type,
    last_named_parameter: Option<usize>,
    target: Target,
    records: &'s RecordScope<'a, 's>,
}

impl<'a> FunctionBody<'a, '_> {
    fn return_value(
        &mut self,
        keyword: &Lexeme,
        value: Option<parse::Expression<'a>>,
    ) -> Result<Option<Expression<'a>>, Diagnostic> {
        // N1570 6.8.6.4: return expressions use assignment conversion.
        match (value, &self.result.kind) {
            (None, Kind::Void) => Ok(None),
            (None, _) => Err(keyword.error("expected a return value in a non-void function")),
            (Some(expr), Kind::Void) => Err(expr.site.error("void function cannot return a value")),
            (Some(expr), _) => {
                let expr = self.expression(expr)?;
                if !assignable(self.result, &expr, self.target) {
                    return Err(expr
                        .site
                        .error(format!("cannot return {} as {}", expr.ty, self.result)));
                }
                Ok(Some(expr))
            }
        }
    }

    fn expression(
        &mut self,
        expression: parse::Expression<'a>,
    ) -> Result<Expression<'a>, Diagnostic> {
        use parse::ExpressionKind as Syntax;
        let site = expression.site;
        let (kind, mut ty) = match expression.kind {
            Syntax::Identifier => {
                let name = site.text.as_ref();
                if let Some(id) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
                    let local = &self.locals[*id];
                    let kind = match local.storage {
                        LocalStorage::ThreadExtern(_) => ExpressionKind::ThreadLocal(name),
                        LocalStorage::Function(_) => ExpressionKind::Identifier(name),
                        _ => ExpressionKind::Local(*id),
                    };
                    (kind, local.object.ty.clone())
                } else {
                    let symbol = self
                        .symbols
                        .get(name)
                        .ok_or_else(|| site.error(format!("undeclared identifier '{name}'")))?;
                    let kind = if symbol.thread_local {
                        ExpressionKind::ThreadLocal(name)
                    } else {
                        ExpressionKind::Identifier(name)
                    };
                    (kind, symbol.ty.clone())
                }
            }
            Syntax::Enumerator(value) => (
                ExpressionKind::Integer(value as u64),
                Type::new(Kind::Integer(Integer::Int)),
            ),
            Syntax::Number => {
                if let Some((bits, ty)) = float::literal(&site.text, self.target) {
                    (ExpressionKind::Float(bits), Type::new(ty))
                } else {
                    let (value, integer) = integer_literal(site, self.target)?;
                    (
                        ExpressionKind::Integer(value),
                        Type::new(Kind::Integer(integer)),
                    )
                }
            }
            Syntax::FltRounds => (
                ExpressionKind::FltRounds,
                Type::new(Kind::Integer(Integer::Int)),
            ),
            Syntax::Character => {
                let value = pp::character_bits(&site.text, self.target)
                    .ok_or_else(|| site.error("invalid character constant"))?;
                let ty = match site.text.as_bytes()[0] {
                    b'u' => Integer::UnsignedShort,
                    b'U' => Integer::UnsignedInt,
                    b'L' if self.target == Target::Aarch64 => Integer::UnsignedInt,
                    _ => Integer::Int,
                };
                (ExpressionKind::Integer(value), Type::new(Kind::Integer(ty)))
            }
            Syntax::Unary(op, expr) => {
                let expr = self.expression(*expr)?;
                let ty = expr.value_type();
                let result = match (op, &ty.kind) {
                    (Unary::Plus | Unary::Minus | Unary::Complement, Kind::Integer(integer)) => {
                        Type::new(Kind::Integer(expr.promoted_integer(*integer)))
                    }
                    (Unary::Plus | Unary::Minus, Kind::Float(_) | Kind::LongDouble) => ty,
                    (Unary::Address, _) if expr.bit_field().is_some() => {
                        return Err(site.error("cannot take the address of a bit-field"));
                    }
                    (Unary::Address, _)
                        if expr.lvalue() || matches!(expr.ty.kind, Kind::Function { .. }) =>
                    {
                        expr.ty.clone().pointer_to()
                    }
                    (Unary::Not, _) if is_scalar(&ty) => Type::new(Kind::Integer(Integer::Int)),
                    (Unary::Dereference, Kind::Pointer(pointee)) if pointee.kind != Kind::Void => {
                        pointee.as_ref().clone()
                    }
                    _ => return Err(site.error(format!("invalid unary operand of type {ty}"))),
                };
                (ExpressionKind::Unary(op, Box::new(expr)), result)
            }
            Syntax::Binary(op, left, right) => {
                let left = self.expression(*left)?;
                let right = self.expression(*right)?;
                let (left_type, right_type, result) = binary_types(op, &left, &right, self.target)
                    .ok_or_else(|| {
                        site.error(format!(
                            "invalid binary operands of type {} and {}",
                            left.ty, right.ty
                        ))
                    })?;
                (
                    ExpressionKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                        left_type,
                        right_type,
                    },
                    result,
                )
            }
            Syntax::Comma(left, right) => {
                let left = self.expression(*left)?;
                let right = self.expression(*right)?;
                let ty = right.value_type();
                (ExpressionKind::Comma(Box::new(left), Box::new(right)), ty)
            }
            Syntax::Cast(ty, expr) => {
                let expr = self.expression(*expr)?;
                let source = expr.converted_type().unwrap_or_else(|| expr.ty.clone());
                if ty.kind != Kind::Void
                    && (!is_scalar(&ty)
                        || !is_scalar(&source)
                        || matches!(
                            (&ty.kind, &source.kind),
                            (Kind::Pointer(_), Kind::Float(_) | Kind::LongDouble)
                                | (Kind::Float(_) | Kind::LongDouble, Kind::Pointer(_))
                        ))
                {
                    return Err(site.error(format!("cannot cast {source} to {ty}")));
                }
                (ExpressionKind::Cast(Box::new(expr)), ty)
            }
            Syntax::CompoundLiteral(ty, initializer) => {
                self.compound_literal(site, ty, *initializer)?
            }
            Syntax::Alignof(ty) => size_constant(type_alignment(&ty, site)?),
            Syntax::Offsetof(ty, path) => size_constant(self.offsetof(site, ty, path)?),
            Syntax::Generic(control, associations) => {
                return self.generic(site, *control, associations)
            }
            Syntax::SizeofType(ty) => sizeof(site, &ty)?,
            Syntax::Sizeof(expr) => {
                let expr = self.unevaluated(*expr)?;
                if expr.bit_field().is_some() {
                    return Err(site.error("sizeof cannot be applied to a bit-field"));
                }
                sizeof(site, &expr.ty)?
            }
            Syntax::Conditional(condition, yes, no) => {
                let condition = self.condition(*condition)?;
                let yes = self.expression(*yes)?;
                let no = self.expression(*no)?;
                let ty = conditional_type(&yes, &no, self.target)
                    .ok_or_else(|| site.error("incompatible conditional operands"))?;
                (
                    ExpressionKind::Conditional {
                        condition: Box::new(condition),
                        yes: Box::new(yes),
                        no: Box::new(no),
                    },
                    ty,
                )
            }
            Syntax::Member {
                base,
                name,
                indirect,
            } => {
                let mut base = self.expression(*base)?;
                if indirect {
                    let Kind::Pointer(mut pointee) = base.value_type().kind else {
                        return Err(site.error("'->' requires a pointer to a record"));
                    };
                    self.records.resolve(&mut pointee, site);
                    base = Expression {
                        site,
                        ty: *pointee,
                        kind: ExpressionKind::Unary(Unary::Dereference, Box::new(base)),
                    };
                }
                let (offset, ty, bit_field) = self.record_member(&base.ty, name)?;
                (
                    ExpressionKind::Member {
                        base: Box::new(base),
                        offset,
                        bit_field,
                    },
                    ty,
                )
            }
            Syntax::Assignment(op, left, right) => {
                let left = self.expression(*left)?;
                let right = self.expression(*right)?;
                if !left.modifiable(self.records.records) {
                    return Err(left.site.error("assignment requires a modifiable lvalue"));
                }
                let operation = if let Some(op) = op {
                    let (a, b, result) = binary_types(op, &left, &right, self.target)
                        .ok_or_else(|| site.error("invalid compound assignment operands"))?;
                    // N1570 6.5.16.2p1 allows pointer arithmetic here only
                    // when the left operand is the pointer and the right is an integer.
                    if (matches!(left.ty.kind, Kind::Pointer(_))
                        || matches!(result.kind, Kind::Pointer(_)))
                        && left.ty.kind != result.kind
                    {
                        return Err(site.error("invalid compound pointer assignment"));
                    }
                    Some((op, a, b))
                } else {
                    if !assignable(&left.ty, &right, self.target) {
                        return Err(right
                            .site
                            .error(format!("cannot assign {} to {}", right.ty, left.ty)));
                    }
                    None
                };
                let ty = left.ty.unqualified();
                (
                    ExpressionKind::Assignment {
                        left: Box::new(left),
                        right: Box::new(right),
                        operation,
                    },
                    ty,
                )
            }
            Syntax::Update {
                operand,
                increment,
                postfix,
            } => {
                let operand = self.expression(*operand)?;
                if !is_scalar(&operand.ty)
                    || !operand.modifiable(self.records.records)
                    || matches!(&operand.ty.kind,
                    Kind::Pointer(ty) if ty.layout().is_none_or(|layout| layout.size == 0))
                {
                    return Err(
                        site.error("increment or decrement requires a modifiable scalar lvalue")
                    );
                }
                let ty = operand.ty.unqualified();
                (
                    ExpressionKind::Update {
                        operand: Box::new(operand),
                        increment,
                        postfix,
                    },
                    ty,
                )
            }
            Syntax::String(tokens) => {
                let mut bytes = Vec::new();
                for token in tokens {
                    decode_string(token, &mut bytes)?;
                }
                // N1570 6.4.5p5–p6: concatenate, then append one terminating byte.
                bytes.push(0);
                let element = Type::new(Kind::Integer(Integer::Char));
                let ty = Type::new(Kind::Array(Box::new(element), bytes.len()));
                (ExpressionKind::String(bytes), ty)
            }
            Syntax::VaArg(list, ty) => self.va_arg(site, *list, ty)?,
            Syntax::Call {
                callee,
                arguments,
                close,
            } => self.call(*callee, arguments, close)?,
        };
        self.records.resolve(&mut ty, site);
        Ok(Expression { site, kind, ty })
    }

    fn call(
        &mut self,
        callee: parse::Expression<'a>,
        arguments: Vec<parse::Expression<'a>>,
        close: &Lexeme,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
        if matches!(callee.kind, parse::ExpressionKind::Identifier) {
            if let Some(op) = AtomicOp::from_name(&callee.site.text) {
                return self.atomic_builtin(callee.site, op, arguments);
            }
        }
        if matches!(callee.kind, parse::ExpressionKind::Identifier)
            && matches!(
                callee.site.text.as_ref(),
                "__builtin_va_start" | "__builtin_va_copy" | "__builtin_va_end"
            )
        {
            return self.va_control(callee.site, arguments);
        }
        let callee = self.expression(callee)?;
        let converted = callee.converted_type();
        let ty = converted.as_ref().unwrap_or(&callee.ty);
        let Kind::Pointer(function) = &ty.kind else {
            return Err(callee.site.error("called expression is not a function"));
        };
        let Kind::Function {
            result,
            params,
            variadic,
            prototype,
        } = &function.kind
        else {
            return Err(callee.site.error("called expression is not a function"));
        };

        if matches!(result.kind, Kind::Record(record) if record.layout.is_none()) {
            return Err(close.error("call has an incomplete record result"));
        }
        // N1570 6.5.2.2p2, p5 and p7: check the prototype and convert each
        // argument as if assigned to its unqualified parameter type.
        let params = if *prototype { params.as_slice() } else { &[] };
        let mut converted_arguments = Vec::new();
        for (index, argument) in arguments.into_iter().enumerate() {
            if *prototype && index >= params.len() && !variadic {
                return Err(argument.site.error("too many arguments"));
            }
            let expr = self.expression(argument)?;
            let ty = if let Some(parameter) = params.get(index) {
                if !assignable(parameter, &expr, self.target) {
                    return Err(expr
                        .site
                        .error(format!("cannot pass {} as {parameter}", expr.ty)));
                }
                parameter.clone()
            } else {
                let ty = expr.value_type();
                // N1570 6.5.2.2p6–7: trailing arguments undergo the default promotions.
                match ty.kind {
                    Kind::Integer(integer) => {
                        Type::new(Kind::Integer(expr.promoted_integer(integer)))
                    }
                    _ if is_scalar(&ty) => ty.default_argument_type(),
                    Kind::ShortVector { .. } => ty,
                    Kind::Record(record) if record.layout.is_some() => ty,
                    _ => {
                        return Err(expr
                            .site
                            .error("expected a scalar or complete record argument"))
                    }
                }
            };
            converted_arguments.push(Argument { expr, ty });
        }
        if converted_arguments.len() < params.len() {
            return Err(close.error("too few arguments"));
        }
        let result = result.as_ref().clone();
        Ok((
            ExpressionKind::Call {
                callee: Box::new(callee),
                arguments: converted_arguments,
            },
            result,
        ))
    }
}

fn is_scalar(ty: &Type) -> bool {
    is_arithmetic(ty) || matches!(ty.kind, Kind::Pointer(_))
}

fn is_arithmetic(ty: &Type) -> bool {
    matches!(
        ty.kind,
        Kind::Integer(_) | Kind::Float(_) | Kind::LongDouble
    )
}

// N1570 6.3.1.8: arithmetic type for both operands after promotion.
fn common_integer(left: Integer, right: Integer, target: Target) -> Integer {
    use Integer::*;
    fn rank(integer: Integer) -> u8 {
        match integer {
            LongLong | UnsignedLongLong => 2,
            Long | UnsignedLong => 1,
            _ => 0,
        }
    }
    let (higher, lower) = if rank(left) >= rank(right) {
        (left, right)
    } else {
        (right, left)
    };
    if higher.is_signed(target) == lower.is_signed(target)
        || !higher.is_signed(target)
        || higher.layout().size > lower.layout().size
    {
        return higher;
    }
    match higher {
        LongLong => UnsignedLongLong,
        Long => UnsignedLong,
        _ => UnsignedInt,
    }
}

fn binary_types(
    op: Binary,
    left: &Expression<'_>,
    right: &Expression<'_>,
    target: Target,
) -> Option<(Type, Type, Type)> {
    use Binary::*;
    let left_value = left;
    let right_value = right;
    let left = left.value_type();
    let right = right.value_type();
    let int = Type::new(Kind::Integer(Integer::Int));
    if matches!(op, LogicalAnd | LogicalOr) {
        return (is_scalar(&left) && is_scalar(&right)).then_some((left, right, int));
    }
    let compare = matches!(
        op,
        Less | LessEqual | Greater | GreaterEqual | Equal | NotEqual
    );
    if let Some(ty) = pointer_binary(op, left_value, right_value, &left, &right, target) {
        return Some(ty);
    }
    let common = match (&left.kind, &right.kind) {
        (Kind::Integer(a), Kind::Integer(b)) if matches!(op, ShiftLeft | ShiftRight) => {
            let left = Type::new(Kind::Integer(left_value.promoted_integer(*a)));
            let right = Type::new(Kind::Integer(right_value.promoted_integer(*b)));
            return Some((left.clone(), right, left));
        }
        (Kind::Integer(a), Kind::Integer(b)) => Type::new(Kind::Integer(common_integer(
            left_value.promoted_integer(*a),
            right_value.promoted_integer(*b),
            target,
        ))),
        _ if is_arithmetic(&left)
            && is_arithmetic(&right)
            && (compare || matches!(op, Add | Subtract | Multiply | Divide)) =>
        {
            let kind = if left.kind == Kind::LongDouble || right.kind == Kind::LongDouble {
                Kind::LongDouble
            } else if left.kind == Kind::Float(Float::Double)
                || right.kind == Kind::Float(Float::Double)
            {
                Kind::Float(Float::Double)
            } else {
                Kind::Float(Float::Single)
            };
            Type::new(kind)
        }
        _ => return None,
    };
    Some((
        common.clone(),
        common.clone(),
        if compare { int } else { common },
    ))
}

fn sizeof<'a>(site: &Lexeme, ty: &Type) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
    let size = ty
        .layout()
        .filter(|layout| layout.size != 0)
        .ok_or_else(|| site.error("sizeof requires a complete object type"))?
        .size;
    Ok(size_constant(size))
}

fn size_constant<'a>(value: usize) -> (ExpressionKind<'a>, Type) {
    (
        ExpressionKind::Integer(value as u64),
        Type::new(Kind::Integer(Integer::UnsignedLong)),
    )
}

fn pointer_binary(
    op: Binary,
    a: &Expression<'_>,
    b: &Expression<'_>,
    left: &Type,
    right: &Type,
    target: Target,
) -> Option<(Type, Type, Type)> {
    use Binary::*;
    let int = || Type::new(Kind::Integer(Integer::Int));
    let index = || Type::new(Kind::Integer(Integer::Long));
    let sized = |ty: &Type| matches!(&ty.kind, Kind::Pointer(to) if to.layout().is_some_and(|layout| layout.size != 0));
    match (&left.kind, &right.kind) {
        (Kind::Pointer(_), Kind::Integer(_)) if matches!(op, Add | Subtract) && sized(left) => {
            Some((left.clone(), index(), left.clone()))
        }
        (Kind::Integer(_), Kind::Pointer(_)) if op == Add && sized(right) => {
            Some((index(), right.clone(), right.clone()))
        }
        (Kind::Pointer(a), Kind::Pointer(b))
            if op == Subtract
                && a.kind == b.kind
                && a.qualifiers.atomic == b.qualifiers.atomic
                && sized(left) =>
        {
            Some((left.clone(), right.clone(), index()))
        }
        (Kind::Pointer(a), Kind::Pointer(b))
            if matches!(op, Less | LessEqual | Greater | GreaterEqual)
                && a.kind == b.kind
                && a.qualifiers.atomic == b.qualifiers.atomic =>
        {
            Some((left.clone(), right.clone(), int()))
        }
        _ if matches!(op, Equal | NotEqual) => {
            let ptr = compatible_pointer(a, b, target)?;
            Some((ptr.clone(), ptr, int()))
        }
        _ => None,
    }
}

fn compatible_pointer(
    left: &Expression<'_>,
    right: &Expression<'_>,
    target: Target,
) -> Option<Type> {
    let a = left.value_type();
    let b = right.value_type();
    match (&a.kind, &b.kind) {
        (Kind::Pointer(a), Kind::Pointer(b)) => {
            let kind = if a
                .unqualified_except_atomic()
                .compatible(&b.unqualified_except_atomic())
            {
                if has_prototype(b) {
                    b.kind.clone()
                } else {
                    a.kind.clone()
                }
            } else if a.kind == Kind::Void && !matches!(b.kind, Kind::Function { .. })
                || b.kind == Kind::Void && !matches!(a.kind, Kind::Function { .. })
            {
                Kind::Void
            } else {
                return None;
            };
            Some(
                Type {
                    kind,
                    qualifiers: crate::types::Qualifiers {
                        constant: a.qualifiers.constant || b.qualifiers.constant,
                        volatile: a.qualifiers.volatile || b.qualifiers.volatile,
                        atomic: a.qualifiers.atomic && b.qualifiers.atomic,
                        restricted: a.qualifiers.restricted || b.qualifiers.restricted,
                    },
                }
                .pointer_to(),
            )
        }
        (Kind::Pointer(_), Kind::Integer(_)) if is_null(right, target) => Some(a),
        (Kind::Integer(_), Kind::Pointer(_)) if is_null(left, target) => Some(b),
        _ => None,
    }
}

fn conditional_type(yes: &Expression<'_>, no: &Expression<'_>, target: Target) -> Option<Type> {
    if yes.ty.kind == Kind::Void && no.ty.kind == Kind::Void {
        return Some(yes.ty.clone());
    }
    if matches!(yes.ty.kind, Kind::Record(_) | Kind::ShortVector { .. })
        && yes.ty.kind == no.ty.kind
    {
        return Some(yes.ty.unqualified());
    }
    compatible_pointer(yes, no, target).or_else(|| {
        if is_arithmetic(&yes.ty) && is_arithmetic(&no.ty) {
            binary_types(Binary::Add, yes, no, target).map(|(_, _, result)| result)
        } else {
            None
        }
    })
}

fn is_null(expr: &Expression<'_>, target: Target) -> bool {
    matches!(expr.ty.kind, Kind::Integer(_)) && body::constant_integer(expr, target) == Some(0)
}

// N1570 6.5.16.1p1 and 6.3.2.3p3 for assignment and null pointers.
fn assignable(destination: &Type, expr: &Expression<'_>, target: Target) -> bool {
    let converted = expr.converted_type();
    let source = converted.as_ref().unwrap_or(&expr.ty);
    match (&destination.kind, &source.kind) {
        _ if is_arithmetic(destination) && is_arithmetic(source) => true,
        (Kind::Integer(Integer::Bool), Kind::Pointer(_)) => true,
        (Kind::Record(to), Kind::Record(from)) => to == from && to.layout.is_some(),
        (to @ Kind::ShortVector { .. }, from) => to == from,
        (Kind::Pointer(_), Kind::Integer(_)) => is_null(expr, target),
        (Kind::Pointer(to), Kind::Pointer(from)) => {
            if !to.qualifiers.contains(from.qualifiers) {
                return false;
            }
            to.unqualified_except_atomic()
                .compatible(&from.unqualified_except_atomic())
                || (to.kind == Kind::Void && !matches!(from.kind, Kind::Function { .. }))
                || (from.kind == Kind::Void && !matches!(to.kind, Kind::Function { .. }))
        }
        _ => false,
    }
}

// N1570 6.4.4.1p5: choose the first candidate that can represent the value.
fn integer_literal(token: &Lexeme, target: Target) -> Result<(u64, Integer), Diagnostic> {
    let constant =
        pp::integer_constant(&token.text).ok_or_else(|| token.error("invalid integer constant"))?;
    use Integer::*;
    let candidates = [
        (Int, UnsignedInt),
        (Long, UnsignedLong),
        (LongLong, UnsignedLongLong),
    ];
    for (signed, unsigned) in candidates.into_iter().skip(constant.longs) {
        if !constant.unsigned && signed.represents(constant.value, target) {
            return Ok((constant.value, signed));
        }
        if (constant.unsigned || !constant.decimal) && unsigned.represents(constant.value, target) {
            return Ok((constant.value, unsigned));
        }
    }
    Err(token.error("integer constant does not fit an available type"))
}

// N1570 5.1.1.2 phases 5–6 and 6.4.5. Ordinary strings use UTF-8;
// numeric escapes encode one byte, including an embedded null.
pub(super) fn decode_string(token: &Lexeme, bytes: &mut Vec<u8>) -> Result<(), Diagnostic> {
    let body = token
        .text
        .strip_prefix('"')
        .and_then(|body| body.strip_suffix('"'))
        .ok_or_else(|| token.error("expected an ordinary string literal"))?;
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        let ch = if ch == '\\' {
            let universal = matches!(chars.peek(), Some('u' | 'U'));
            let code =
                pp::escape(&mut chars).ok_or_else(|| token.error("invalid string escape"))?;
            if code.numeric {
                let byte = u8::try_from(code.value)
                    .map_err(|_| token.error("string escape exceeds unsigned char range"))?;
                bytes.push(byte);
                continue;
            }
            let ch =
                char::from_u32(code.value).ok_or_else(|| token.error("invalid string escape"))?;
            // N1570 6.4.3p2 limits universal character names below U+00A0.
            if universal && ch < '\u{a0}' && !matches!(ch, '$' | '@' | '`') {
                return Err(token.error("invalid universal character name"));
            }
            ch
        } else {
            ch
        };
        let mut encoded = [0; 4];
        bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
    }
    Ok(())
}
