//! Typed translation units for the hello subset of N1570.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::parse::{self, Storage, SymbolAttributes};
use crate::pp::{self, Diagnostic, Lexeme, Position, Preprocessed, Standard, Target};
use crate::types::{BitField, Float, Integer, Kind, Record, Type};
pub use parse::{Binary, Unary};

mod atomic;
mod body;
mod builtins;
mod c11;
mod dump;
mod float;
mod inline_asm;
mod thread;
mod varargs;
mod vla;

pub(crate) use body::{canonical, constant_integer};
pub(super) use c11::{check_static_assertion, type_alignment};

pub use atomic::AtomicOp;
pub use inline_asm::{AsmRegister, InlineAsm};
pub(crate) use inline_asm::asm_register;
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
    pub(crate) symbol_attributes: BTreeMap<&'a str, SymbolAttributes>,
    pub(crate) aliases: BTreeMap<&'a str, Linkage>,
    rounding_math: bool,
}

impl TranslationUnit<'_> {
    /// Consult before any runtime FP optimization, including constant
    /// propagation, CSE, and code motion. When true, preserve operations
    /// whose rounding or exceptions are observable in the live environment.
    /// Preserve FP grouping in either case (N1570 5.1.2.3p14, 7.6.1 and F.9).
    pub fn preserves_float_environment(&self) -> bool {
        // As documented by Preprocessed::tokens, bcc currently uses
        // FENV_ACCESS ON semantics throughout the translation unit.
        const CONSERVATIVE_FENV_ACCESS: bool = true;
        self.rounding_math || CONSERVATIVE_FENV_ACCESS
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
    pub returns_twice: bool,
    pub has_alloca: bool,
    pub(crate) has_statement_expressions: bool,
    pub(crate) jump_regions: HashMap<usize, usize>,
    pub(crate) vla_targets: HashMap<usize, usize>,
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

#[derive(Clone, Debug)]
pub struct Initializer<'a> {
    pub offset: usize,
    pub bit_field: Option<BitField>,
    pub value: InitValue<'a>,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub enum InitValue<'a> {
    Expression(Expression<'a>),
    Zero,
}

#[derive(Clone, Debug)]
pub struct ObjectInitializer<'a> {
    pub values: Vec<Initializer<'a>>,
    pub zero_bytes: usize,
}

#[derive(Clone, Debug)]
pub enum Statement<'a> {
    Block(Vec<Self>),
    Empty,
    Initialize {
        local: usize,
        initializer: ObjectInitializer<'a>,
    },
    Expression(Expression<'a>),
    InlineAsm(InlineAsm<'a>),
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
        cases: Vec<(Option<CaseRange>, usize)>,
        body: Box<Self>,
        break_to: usize,
    },
    Label(usize, Box<Self>),
    Jump(usize, Vec<usize>),
    SaveVlaStack(usize),
    RestoreVlaStack(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct CaseRange {
    pub low: u64,
    pub high: u64,
}

#[derive(Clone, Debug)]
pub struct Expression<'a> {
    pub(crate) site: &'a Lexeme,
    pub kind: ExpressionKind<'a>,
    pub ty: Type,
}

#[derive(Clone, Debug)]
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
    StatementExpression {
        region: usize,
        statements: Vec<Statement<'a>>,
        value: Option<Box<Expression<'a>>>,
    },
    Conditional {
        condition: Box<Expression<'a>>,
        yes: Box<Expression<'a>>,
        no: Box<Expression<'a>>,
    },
    OmittedConditional {
        value: Box<Expression<'a>>,
        fallback: Box<Expression<'a>>,
        saved: Box<Expression<'a>>,
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
    Unreachable,
    FloatClass {
        operand: Box<Expression<'a>>,
        // GCC §7.2.1 order: FP_NAN, FP_INFINITE, FP_NORMAL, FP_SUBNORMAL, FP_ZERO.
        values: [i32; 5],
    },
    SignBit(Box<Expression<'a>>),
    Alloca(Box<Expression<'a>>),
    Atomic {
        op: AtomicOp,
        weak: bool,
        arguments: Vec<Argument<'a>>,
    },
    Call {
        callee: Box<Expression<'a>>,
        arguments: Vec<Argument<'a>>,
    },
}

#[derive(Clone, Debug)]
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

    pub(crate) fn lvalue(&self) -> bool {
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
    analyze_with_rounding_math(source, path, target, false)
}

pub(crate) fn analyze_with_rounding_math<'a>(
    source: &'a Preprocessed,
    path: &Path,
    target: Target,
    rounding_math: bool,
) -> Result<TranslationUnit<'a>, Diagnostic> {
    let language = Language {
        standard: source.standard,
        alloca_alias: source.standard.is_gnu() && !source.no_builtin_alloca,
    };
    let mut symbols = HashMap::new();
    let mut symbol_attributes = BTreeMap::<&str, SymbolAttributes>::new();
    let mut attribute_sites = HashMap::new();
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
                    language,
                    &[],
                    &record_scope,
                    &mut block_externs,
                );
                context.static_assertion(assertion)?;
            }
            parse::Declaration::Function(mut declaration) => {
                let site = declaration.name;
                let key = site.text.as_ref();
                let attributes = std::mem::take(&mut declaration.symbol);
                let alias = attributes.alias.is_some();
                if attributes != SymbolAttributes::default() {
                    attribute_sites.insert(key, site);
                    symbol_attributes.entry(key).or_default().merge(attributes, site)?;
                }
                if alias && declaration.body.is_some() {
                    return Err(site.error("function alias cannot have a body"));
                }
                functions.push(function(
                    declaration,
                    alias,
                    &mut symbols,
                    target,
                    language,
                    &record_scope,
                    &mut block_externs,
                )?);
            }
            parse::Declaration::Object(mut variable) => {
                let name = variable.name;
                let key = name.text.as_ref();
                let previous = symbols.get(key);
                let alias = variable.symbol.alias.is_some();
                if variable.symbol != SymbolAttributes::default() {
                    attribute_sites.insert(key, name);
                    symbol_attributes.entry(key).or_default()
                        .merge(std::mem::take(&mut variable.symbol), name)?;
                }
                if alias && (variable.initializer.is_some() || variable.storage != Storage::Extern) {
                    return Err(name.error("object alias requires extern with no initializer"));
                }
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
                        language,
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
                            defined: defined || alias || previous.is_some_and(|symbol| symbol.defined),
                            thread_local: true,
                            alignment: alignment.or(previous.and_then(|prior| prior.alignment)),
                            noreturn: false,
                            returns_twice: false,
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
                        if (explicit || alias) && symbol.defined {
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
                            defined: explicit || alias || previous.is_some_and(|symbol| symbol.defined),
                            alignment: alignment.or(previous.and_then(|prior| prior.alignment)),
                            noreturn: false,
                            returns_twice: false,
                        },
                    );
                    if let Some(initializer) = variable.initializer {
                        let result = Type::new(Kind::Void);
                        let mut context = FunctionBody::new(
                            &symbols,
                            &result,
                            target,
                            language,
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
        if symbol_attributes.get(key).is_some_and(|attrs| attrs.alias.is_some()) {
            return Err(site.error("object alias cannot have a tentative definition"));
        }
        let symbol = symbols.get_mut(key).unwrap();
        if symbol.defined {
            continue;
        }
        let mut ty = symbol.ty.clone();
        if let Kind::Array(_, count @ None) = &mut ty.kind {
            *count = Some(1);
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
        function.returns_twice = symbols[function.name].returns_twice;
    }
    for thread in &mut thread_objects.definitions {
        // N1570 6.9.2: a remaining external tentative array with no bound
        // is completed as a one-element array at the end of the unit.
        if let Kind::Array(_, count @ None) = &mut thread.object.ty.kind {
            *count = Some(1);
        }
        thread
            .object
            .ty
            .resolve_records(&|id| records[id].definition.as_ref().map(|d| d.layout));
        thread::check_layout(&thread.object.ty, thread.object.site)?;
    }
    let aliases = validate_symbol_attributes(&symbol_attributes, &attribute_sites, &symbols)?;
    Ok(TranslationUnit {
        functions,
        objects,
        static_objects,
        static_literals,
        thread_objects: thread_objects.definitions,
        records,
        symbol_attributes,
        aliases,
        rounding_math,
    })
}

// GNU Common Attributes: aliases denote definitions in this translation
// unit and share the target's type and storage. gABI §§5.2–5.4 supply
// external linkage for weak binding and non-default visibility.
fn validate_symbol_attributes<'a>(
    attributes: &BTreeMap<&'a str, SymbolAttributes>,
    sites: &HashMap<&str, &Lexeme>,
    symbols: &HashMap<&str, Symbol>,
) -> Result<BTreeMap<&'a str, Linkage>, Diagnostic> {
    let mut aliases = BTreeMap::new();
    for (&name, attrs) in attributes {
        let symbol = &symbols[name];
        let site = sites[name];
        if symbol.linkage == Linkage::Internal && (attrs.weak || attrs.visibility.is_some()) {
            return Err(site.error("weak and visibility attributes require external linkage"));
        }
        let Some(target) = &attrs.alias else {
            continue;
        };
        if attrs.section.is_some() {
            return Err(site.error("an alias takes its section from its target"));
        }
        let target_symbol = symbols.get(target.as_str()).filter(|symbol| symbol.defined)
            .ok_or_else(|| site.error(format!("alias target '{target}' is not defined in this translation unit")))?;
        if symbol.thread_local != target_symbol.thread_local || !symbol.ty.compatible(&target_symbol.ty) {
            return Err(site.error(format!("alias target '{target}' has an incompatible type")));
        }
        let mut current = target.as_str();
        let mut seen = HashSet::from([name]);
        while let Some(next) = attributes.get(current).and_then(|attrs| attrs.alias.as_deref()) {
            if !seen.insert(current) {
                return Err(site.error("alias cycle"));
            }
            current = next;
        }
        aliases.insert(name, symbol.linkage);
    }
    Ok(aliases)
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
    returns_twice: bool,
}

/// Evaluate a layout constant while parsing a record. The parser's record
/// table already reflects which tags are complete at this point.
pub(super) struct ParserExpressionScope<'a, 'r> {
    target: Target,
    standard: Standard,
    records: &'r [Record<'a>],
    symbols: HashMap<&'a str, Symbol>,
}

impl<'a, 'r> ParserExpressionScope<'a, 'r> {
    pub(super) fn new<'t>(
        target: Target,
        standard: Standard,
        records: &'r [Record<'a>],
        names: impl Iterator<Item = (&'a str, Option<&'t Type>)>,
    ) -> Self {
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
                        returns_twice: false,
                    },
                );
            } else {
                symbols.remove(name);
            }
        }
        Self {
            target,
            standard,
            records,
            symbols,
        }
    }

    fn with_body<T>(
        &self,
        evaluate: impl FnOnce(&mut FunctionBody<'a, '_>) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        let record_scope = RecordScope {
            records: self.records,
            token_order: None,
        };
        let result = Type::new(Kind::Void);
        let mut block_externs = HashMap::new();
        let mut context = FunctionBody::new(
            &self.symbols,
            &result,
            self.target,
            Language { standard: self.standard, alloca_alias: false },
            &[],
            &record_scope,
            &mut block_externs,
        );
        evaluate(&mut context)
    }

    fn expression(&self, expression: parse::Expression<'a>) -> Result<Expression<'a>, Diagnostic> {
        self.with_body(|body| body.expression(expression))
    }

}

// N1570 §6.7.9p22 completes an array at the end of its initializer.
// Later parser-time constant evaluation (e.g. sizeof in an enum) needs it.
pub(super) fn complete_initializer_array<'a>(
    site: &'a Lexeme,
    mut ty: Type,
    initializer: parse::Initializer<'a>,
    scope: ParserExpressionScope<'a, '_>,
) -> Result<Type, Diagnostic> {
    scope.with_body(|body| body.object_initializer(site, &mut ty, initializer, false))?;
    Ok(ty)
}

pub(super) fn layout_constant<'a>(
    expression: parse::Expression<'a>,
    scope: ParserExpressionScope<'a, '_>,
) -> Result<u64, Diagnostic> {
    let expr = scope.expression(expression)?;
    body::constant_integer(&expr, scope.target)
        .ok_or_else(|| expr.site.error("expected an integer constant expression"))
}

// N1570 §6.7.2.2: each enumerator has an integer constant expression.
// Preserve the initializer's signedness before selecting the enum type.
pub(super) fn enumerator_constant<'a>(
    expression: parse::Expression<'a>,
    scope: ParserExpressionScope<'a, '_>,
) -> Result<i128, Diagnostic> {
    let expr = scope.expression(expression)?;
    let bits = body::constant_integer(&expr, scope.target)
        .ok_or_else(|| expr.site.error("expected an integer constant expression"))?;
    Ok(if matches!(expr.ty.kind, Kind::Integer(ty) if ty.is_signed(scope.target)) {
        i128::from(bits as i64)
    } else {
        i128::from(bits)
    })
}

pub(super) fn typeof_expression<'a>(
    expression: parse::Expression<'a>,
    scope: ParserExpressionScope<'a, '_>,
) -> Result<Type, Diagnostic> {
    let expr = scope.expression(expression)?;
    if expr.bit_field().is_some() {
        return Err(expr.site.error("typeof cannot be applied to a bit-field"));
    }
    Ok(expr.ty)
}

fn function<'a>(
    declaration: parse::Function<'a>,
    alias: bool,
    symbols: &mut HashMap<&'a str, Symbol>,
    target: Target,
    language: Language,
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
        returns_twice,
        storage,
        body,
        ..
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
        if symbol.defined && (defined || alias) {
            return Err(name.error("function already defined"));
        }
    }
    let visible_type = previous
        .filter(|symbol| !prototype && (has_prototype(&symbol.ty) || symbol.defined && !defined))
        .map_or(&ty, |symbol| &symbol.ty)
        .clone();
    let noreturn = noreturn || previous.is_some_and(|symbol| symbol.noreturn);
    let returns_twice = returns_twice || previous.is_some_and(|symbol| symbol.returns_twice);
    symbols.insert(
        &name.text,
        Symbol {
            ty: visible_type,
            linkage,
            defined: defined || alias || previous.is_some_and(|s| s.defined),
            thread_local: false,
            alignment: None,
            noreturn,
            returns_twice,
        },
    );
    let (body, locals, has_alloca, has_statement_expressions, jump_regions, vla_targets) =
        if let Some(statements) = body {
            let mut context = FunctionBody::new(
                symbols,
                &result,
                target,
                language,
                &params,
                records,
                block_externs,
            );
            if variadic {
                context.last_named_parameter = params.len().checked_sub(1);
            }
            let body = context.statements(statements)?;
            context.validate_labels()?;
            (
                Some(body),
                context.locals,
                context.has_alloca,
                context.has_statement_expressions,
                context.jump_regions,
                context.vla_targets,
            )
        } else {
            (None, Vec::new(), false, false, HashMap::new(), HashMap::new())
        };
    Ok(Function {
        name: &name.text,
        site: &name.site,
        ty,
        linkage,
        params,
        noreturn,
        returns_twice,
        has_alloca,
        has_statement_expressions,
        jump_regions,
        vla_targets,
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

#[derive(Clone, Copy)]
struct Language {
    standard: Standard,
    alloca_alias: bool,
}

struct FunctionBody<'a, 's> {
    symbols: &'s HashMap<&'a str, Symbol>,
    // Declarations with external linkage must agree even when they occur
    // in blocks that are outside each other's lexical scope.
    block_externs: &'s mut HashMap<&'a str, Type>,
    locals: Vec<Local<'a>>,
    has_alloca: bool,
    alloca_alias: bool,
    static_initialization: bool,
    // Present only while analyzing file-scope initializers.
    static_literals: Option<(usize, Vec<StaticLiteral<'a>>)>,
    scopes: Vec<HashMap<&'a str, usize>>,
    labels: HashMap<&'a str, body::Label<'a>>,
    next_label: usize,
    exit: Option<usize>,
    repeat: Option<usize>,
    switches: Vec<body::Switch>,
    statement_regions: Vec<usize>,
    has_statement_expressions: bool,
    jump_regions: HashMap<usize, usize>,
    vla_scopes: Vec<usize>,
    vla_targets: HashMap<usize, usize>,
    vla_locals: HashMap<usize, vla::Array>,
    result: &'s Type,
    last_named_parameter: Option<usize>,
    target: Target,
    standard: Standard,
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
                if !assignable(self.result, &expr, self.target, self.standard) {
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
                if let Some(builtin) = builtins::library_builtin(name) {
                    builtin
                } else if let Some(id) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
                    let local = &self.locals[*id];
                    if let Some(array) = self.vla_locals.get(id) {
                        return Ok(array.reference(site, *id, &local.object.ty));
                    }
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
            Syntax::Enumerator(value, integer) => (
                ExpressionKind::Integer(value),
                Type::new(Kind::Integer(integer)),
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
                    b'L' => wchar_type(self.target),
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
            Syntax::Cast(mut ty, expr) => {
                // WG14 N2244 DR 423, C17 6.5.4p5.
                if self.standard == Standard::C17 {
                    ty.qualifiers = Default::default();
                }
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
                let kind = if self.standard.is_gnu() && self.static_initialization
                    && matches!(ty.kind, Kind::Integer(_))
                {
                    builtins::null_record_offset(&expr, self.target)
                        .map_or_else(|| ExpressionKind::Cast(Box::new(expr)), ExpressionKind::Integer)
                } else {
                    ExpressionKind::Cast(Box::new(expr))
                };
                (kind, ty)
            }
            Syntax::CompoundLiteral(ty, initializer) => {
                self.compound_literal(site, ty, *initializer)?
            }
            Syntax::Alignof(ty) => size_constant(type_alignment(&ty, site)?),
            Syntax::Offsetof(ty, path) => size_constant(self.offsetof(site, ty, path)?),
            Syntax::TypesCompatible(left, right) => (
                ExpressionKind::Integer(u64::from(left.unqualified().compatible(&right.unqualified()))),
                Type::new(Kind::Integer(Integer::Int)),
            ),
            Syntax::Generic(control, associations) => {
                return self.generic(site, *control, associations)
            }
            Syntax::SizeofType(ty) => sizeof(site, &ty)?,
            Syntax::Sizeof(expr) => {
                let expr = self.unevaluated(*expr)?;
                if expr.bit_field().is_some() {
                    return Err(site.error("sizeof cannot be applied to a bit-field"));
                }
                if let Some(size) = self.vla_size(&expr) {
                    (ExpressionKind::Local(size), self.locals[size].object.ty.clone())
                } else {
                    sizeof(site, &expr.ty)?
                }
            }
            Syntax::StatementExpression(statements) => {
                return self.statement_expression(site, statements);
            }
            Syntax::OmittedConditional(value, fallback) => {
                return self.omitted_conditional(site, *value, *fallback);
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
                    if !assignable(&left.ty, &right, self.target, self.standard) {
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
                let (element, bytes, count) = string_literal(&tokens, self.target)?;
                let ty = Type::new(Kind::Array(
                    Box::new(Type::new(Kind::Integer(element))),
                    Some(count),
                ));
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
            let name = callee.site.text.as_ref();
            // The GNU library spelling uses the same caller-frame operation
            // as the explicit builtin. A local function pointer can shadow it.
            if name == "alloca" && self.alloca_alias
                && self.scopes.iter().rev().find_map(|scope| scope.get(name))
                    .is_none_or(|id| matches!(self.locals[*id].storage, LocalStorage::Function(_)))
                && self.symbols.get(name).is_none_or(|symbol| matches!(symbol.ty.kind, Kind::Function { .. }))
            {
                return self.builtin(callee.site, builtins::Builtin::Alloca, arguments);
            }
            if let Some(builtin) = builtins::Builtin::named(name) {
                return self.builtin(callee.site, builtin, arguments);
            }
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
                if !assignable(parameter, &expr, self.target, self.standard) {
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
        .complete_layout()
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
    if matches!(a.kind, Kind::Pointer(_)) && is_null(right, target) {
        return Some(a);
    }
    if matches!(b.kind, Kind::Pointer(_)) && is_null(left, target) {
        return Some(b);
    }
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
                        may_alias: a.qualifiers.may_alias || b.qualifiers.may_alias,
                    },
                }
                .pointer_to(),
            )
        }
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
    // N1570 6.3.2.3p3 includes casting an integer constant expression equal
    // to 0 to void *, including when it is converted to a function pointer.
    let integer = match (&expr.ty.kind, &expr.kind) {
        (Kind::Pointer(pointee), ExpressionKind::Cast(operand))
            if pointee.kind == Kind::Void && pointee.qualifiers == Default::default() =>
        {
            operand
        }
        _ => expr,
    };
    body::constant_integer(integer, target) == Some(0)
}

// N1570 6.5.16.1p1 and 6.3.2.3p3 for assignment and null pointers.
fn assignable(destination: &Type, expr: &Expression<'_>, target: Target, standard: Standard) -> bool {
    let converted = expr.converted_type();
    let source = converted.as_ref().unwrap_or(&expr.ty);
    match (&destination.kind, &source.kind) {
        _ if is_arithmetic(destination) && is_arithmetic(source) => true,
        (Kind::Integer(Integer::Bool), Kind::Pointer(_)) => true,
        (Kind::Record(to), Kind::Record(from)) => to == from && to.layout.is_some(),
        (to @ Kind::ShortVector { .. }, from) => to == from,
        (Kind::Pointer(_), Kind::Integer(_) | Kind::Pointer(_)) if is_null(expr, target) => true,
        (Kind::Pointer(to), Kind::Pointer(from)) => {
            if !to.qualifiers.contains(from.qualifiers) {
                return false;
            }
            to.unqualified_except_atomic()
                .compatible(&from.unqualified_except_atomic())
                || (to.kind == Kind::Void && (standard.is_gnu() || !matches!(from.kind, Kind::Function { .. })))
                || (from.kind == Kind::Void && (standard.is_gnu() || !matches!(to.kind, Kind::Function { .. })))
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

fn wchar_type(target: Target) -> Integer {
    match target {
        Target::X86_64 => Integer::Int,
        Target::Aarch64 => Integer::UnsignedInt,
    }
}

fn string_element(tokens: &[&Lexeme], target: Target) -> Integer {
    if tokens.iter().any(|token| token.text.starts_with("L\"")) {
        wchar_type(target)
    } else {
        Integer::Char
    }
}

pub(super) fn string_literal(
    tokens: &[&Lexeme],
    target: Target,
) -> Result<(Integer, Vec<u8>, usize), Diagnostic> {
    let element = string_element(tokens, target);
    let width = element.layout().size;
    let mut bytes = Vec::new();
    for token in tokens {
        decode_literal(token, width != 1, &mut bytes)?;
    }
    bytes.resize(bytes.len() + width, 0);
    let count = bytes.len() / width;
    Ok((element, bytes, count))
}

// N1570 6.7.9p14–15: ordinary strings initialize character arrays,
// and wide strings initialize arrays with a wchar_t-compatible element type.
pub(super) fn string_initializes(element: &Type, literal: Integer) -> bool {
    matches!(element.kind, Kind::Integer(integer)
        if integer == literal || literal == Integer::Char
            && matches!(integer, Integer::SignedChar | Integer::UnsignedChar))
}

pub(super) fn decode_string(token: &Lexeme, bytes: &mut Vec<u8>) -> Result<(), Diagnostic> {
    decode_literal(token, false, bytes)
}

// N1570 5.1.1.2 phases 5–6 and 6.4.5. The ordinary execution
// encoding is UTF-8 and the wide encoding uses 32-bit Unicode values.
fn decode_literal(token: &Lexeme, wide: bool, bytes: &mut Vec<u8>) -> Result<(), Diagnostic> {
    let spelling = if wide {
        token.text.strip_prefix('L').unwrap_or(&token.text)
    } else {
        &token.text
    };
    let body = spelling
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
                if wide {
                    bytes.extend_from_slice(&code.value.to_le_bytes());
                } else {
                    let byte = u8::try_from(code.value)
                        .map_err(|_| token.error("string escape exceeds unsigned char range"))?;
                    bytes.push(byte);
                }
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
        if wide {
            bytes.extend_from_slice(&u32::from(ch).to_le_bytes());
        } else {
            let mut encoded = [0; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
        }
    }
    Ok(())
}
