//! Typed translation units for the hello subset of N1570.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::parse::{self, Storage};
use crate::pp::{self, Diagnostic, Lexeme, Position, Preprocessed, Target};
use crate::types::{Float, Integer, Kind, Type};
pub use parse::{Binary, Unary};

mod body;
mod dump;
mod float;

#[derive(Debug)]
pub struct TranslationUnit<'a> {
    pub functions: Vec<Function<'a>>,
    pub objects: Vec<Object<'a>>,
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
}

#[derive(Debug)]
pub struct Local<'a> {
    pub object: Object<'a>,
    pub static_storage: bool,
}

#[derive(Debug)]
pub struct Initializer<'a> {
    pub offset: usize,
    pub value: Expression<'a>,
    pub ty: Type,
}

#[derive(Debug)]
pub enum Statement<'a> {
    Block(Vec<Self>),
    Empty,
    Initialize {
        local: usize,
        values: Vec<Initializer<'a>>,
        zero_bytes: usize,
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
    Local(usize),
    Integer(u64),
    Float(u64),
    Unary(Unary, Box<Expression<'a>>),
    Binary {
        op: Binary,
        left: Box<Expression<'a>>,
        right: Box<Expression<'a>>,
        left_type: Type,
        right_type: Type,
    },
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
    String(Vec<u8>),
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

    fn lvalue(&self) -> bool {
        !matches!(self.ty.kind, Kind::Function { .. })
            && matches!(
                self.kind,
                ExpressionKind::Identifier(_)
                    | ExpressionKind::Local(_)
                    | ExpressionKind::String(_)
                    | ExpressionKind::Unary(Unary::Dereference, _)
            )
    }

    fn modifiable(&self) -> bool {
        self.lvalue() && !self.ty.qualifiers.constant && is_scalar(&self.ty)
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
    for declaration in parse::translation_unit(source, path)? {
        match declaration {
            parse::Declaration::Function(declaration) => {
                functions.push(function(declaration, &mut symbols, target)?);
            }
            parse::Declaration::Object(variable) => {
                let name = variable.name;
                if variable.storage != Storage::Extern || variable.initializer.is_some() {
                    return Err(
                        name.error("file-scope object definitions are outside the current subset")
                    );
                }
                if variable.ty.kind == Kind::Void
                    || matches!(variable.ty.kind, Kind::Function { .. })
                {
                    return Err(name.error("invalid object type"));
                }
                if let Some(symbol) = symbols.get(name.text.as_ref()) {
                    if symbol.ty != variable.ty || symbol.linkage != Linkage::External {
                        return Err(name.error("incompatible redeclaration"));
                    }
                }
                symbols.insert(
                    &name.text,
                    Symbol {
                        ty: variable.ty.clone(),
                        linkage: Linkage::External,
                        defined: false,
                    },
                );
                objects.push(Object {
                    name: &name.text,
                    site: &name.site,
                    ty: variable.ty,
                });
            }
        }
    }
    Ok(TranslationUnit { functions, objects })
}

struct Symbol {
    ty: Type,
    linkage: Linkage,
    defined: bool,
}

fn function<'a>(
    declaration: parse::Function<'a>,
    symbols: &mut HashMap<&'a str, Symbol>,
    target: Target,
) -> Result<Function<'a>, Diagnostic> {
    let parse::Function {
        name,
        result,
        params,
        variadic,
        storage,
        body,
    } = declaration;
    let defined = body.is_some();
    if defined && variadic {
        return Err(name.error("variadic function definitions are deferred to 25.7b2"));
    }
    let mut names = HashSet::new();
    let params: Vec<_> = params
        .into_iter()
        .map(|param| {
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
        params: params.iter().map(|param| param.ty.unqualified()).collect(),
        variadic,
    });
    let previous = symbols.get(name.text.as_ref());
    // N1570 6.2.2p3–p5: a declaration with extern linkage rules inherits
    // the linkage of a visible declaration.
    let linkage = match (storage, previous) {
        (Storage::Static, _) => Linkage::Internal,
        (_, Some(symbol)) => symbol.linkage,
        (_, None) => Linkage::External,
    };
    if let Some(symbol) = previous {
        if symbol.ty != ty || symbol.linkage != linkage {
            return Err(name.error("incompatible redeclaration"));
        }
        if symbol.defined && defined {
            return Err(name.error("function already defined"));
        }
    }
    symbols.insert(
        &name.text,
        Symbol {
            ty: ty.clone(),
            linkage,
            defined: defined || previous.is_some_and(|s| s.defined),
        },
    );
    let (body, locals) = if let Some(statements) = body {
        let mut context = FunctionBody::new(symbols, &result, target, &params);
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
        body,
        locals,
    })
}

struct FunctionBody<'a, 's> {
    symbols: &'s HashMap<&'a str, Symbol>,
    locals: Vec<Local<'a>>,
    scopes: Vec<HashMap<&'a str, usize>>,
    labels: HashMap<&'a str, body::Label<'a>>,
    next_label: usize,
    exit: Option<usize>,
    repeat: Option<usize>,
    switches: Vec<body::Switch>,
    result: &'s Type,
    target: Target,
}

impl<'a> FunctionBody<'a, '_> {
    fn return_value(
        &self,
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

    fn expression(&self, expression: parse::Expression<'a>) -> Result<Expression<'a>, Diagnostic> {
        use parse::ExpressionKind as Syntax;
        let site = expression.site;
        let (kind, ty) = match expression.kind {
            Syntax::Identifier => {
                let name = site.text.as_ref();
                if let Some(id) = self.scopes.iter().rev().find_map(|scope| scope.get(name)) {
                    (
                        ExpressionKind::Local(*id),
                        self.locals[*id].object.ty.clone(),
                    )
                } else {
                    let ty = self
                        .symbols
                        .get(name)
                        .map(|symbol| symbol.ty.clone())
                        .ok_or_else(|| site.error(format!("undeclared identifier '{name}'")))?;
                    (ExpressionKind::Identifier(name), ty)
                }
            }
            Syntax::Enumerator(value) => (
                ExpressionKind::Integer(value as u64),
                Type::new(Kind::Integer(Integer::Int)),
            ),
            Syntax::Number => {
                if let Some((bits, ty)) = float::literal(&site.text) {
                    (ExpressionKind::Float(bits), Type::new(Kind::Float(ty)))
                } else {
                    let (value, integer) = integer_literal(site, self.target)?;
                    (
                        ExpressionKind::Integer(value),
                        Type::new(Kind::Integer(integer)),
                    )
                }
            }
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
                        Type::new(Kind::Integer(promote(*integer)))
                    }
                    (Unary::Plus | Unary::Minus, Kind::Float(_)) => ty,
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
            Syntax::Cast(ty, expr) => {
                let expr = self.expression(*expr)?;
                let source = expr.converted_type().unwrap_or_else(|| expr.ty.clone());
                if ty.kind != Kind::Void
                    && (!is_scalar(&ty)
                        || !is_scalar(&source)
                        || matches!(
                            (&ty.kind, &source.kind),
                            (Kind::Pointer(_), Kind::Float(_)) | (Kind::Float(_), Kind::Pointer(_))
                        ))
                {
                    return Err(site.error(format!("cannot cast {source} to {ty}")));
                }
                (ExpressionKind::Cast(Box::new(expr)), ty)
            }
            Syntax::SizeofType(ty) => sizeof(site, &ty)?,
            Syntax::Sizeof(expr) => sizeof(site, &self.expression(*expr)?.ty)?,
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
            Syntax::Assignment(op, left, right) => {
                let left = self.expression(*left)?;
                let right = self.expression(*right)?;
                if !left.modifiable() {
                    return Err(left
                        .site
                        .error("assignment requires a modifiable scalar lvalue"));
                }
                let operation = if let Some(op) = op {
                    let (a, b, result) = binary_types(op, &left, &right, self.target)
                        .ok_or_else(|| site.error("invalid compound assignment operands"))?;
                    if matches!(left.ty.kind, Kind::Pointer(_)) && left.ty.kind != result.kind {
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
                if !operand.modifiable()
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
            Syntax::Call {
                callee,
                arguments,
                close,
            } => self.call(*callee, arguments, close)?,
        };
        Ok(Expression { site, kind, ty })
    }

    fn call(
        &self,
        callee: parse::Expression<'a>,
        arguments: Vec<parse::Expression<'a>>,
        close: &Lexeme,
    ) -> Result<(ExpressionKind<'a>, Type), Diagnostic> {
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
        } = &function.kind
        else {
            return Err(callee.site.error("called expression is not a function"));
        };

        // N1570 6.5.2.2p2, p5 and p7: check the prototype and convert each
        // argument as if assigned to its unqualified parameter type.
        let mut converted_arguments = Vec::new();
        for (index, argument) in arguments.into_iter().enumerate() {
            if index >= params.len() && !variadic {
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
                    Kind::Integer(integer) => Type::new(Kind::Integer(promote(integer))),
                    Kind::Float(Float::Single) => Type::new(Kind::Float(Float::Double)),
                    _ if is_scalar(&ty) => ty,
                    _ => return Err(expr.site.error("expected a scalar variadic argument")),
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
    matches!(
        ty.kind,
        Kind::Integer(_) | Kind::Float(_) | Kind::Pointer(_)
    )
}

fn promote(integer: Integer) -> Integer {
    if integer.layout().size < 4 {
        Integer::Int
    } else {
        integer
    }
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
    let left = promote(left);
    let right = promote(right);
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
            let left = Type::new(Kind::Integer(promote(*a)));
            let right = Type::new(Kind::Integer(promote(*b)));
            return Some((left.clone(), right, left));
        }
        (Kind::Integer(a), Kind::Integer(b)) => {
            Type::new(Kind::Integer(common_integer(*a, *b, target)))
        }
        (Kind::Integer(_) | Kind::Float(_), Kind::Integer(_) | Kind::Float(_))
            if compare || matches!(op, Add | Subtract | Multiply | Divide) =>
        {
            let float = if left.kind == Kind::Float(Float::Double)
                || right.kind == Kind::Float(Float::Double)
            {
                Float::Double
            } else {
                Float::Single
            };
            Type::new(Kind::Float(float))
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
    Ok((
        ExpressionKind::Integer(size as u64),
        Type::new(Kind::Integer(Integer::UnsignedLong)),
    ))
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
            if op == Subtract && a.kind == b.kind && sized(left) =>
        {
            Some((left.clone(), right.clone(), index()))
        }
        (Kind::Pointer(a), Kind::Pointer(b))
            if matches!(op, Less | LessEqual | Greater | GreaterEqual) && a.kind == b.kind =>
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
            let kind = if a.kind == b.kind {
                a.kind.clone()
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
    compatible_pointer(yes, no, target).or_else(|| {
        if matches!(yes.ty.kind, Kind::Integer(_) | Kind::Float(_))
            && matches!(no.ty.kind, Kind::Integer(_) | Kind::Float(_))
        {
            binary_types(Binary::Add, yes, no, target).map(|(_, _, result)| result)
        } else {
            None
        }
    })
}

fn is_null(expr: &Expression<'_>, target: Target) -> bool {
    matches!(expr.ty.kind, Kind::Integer(_)) && body::constant_integer(expr, target) == Some(0)
}

// N1570 6.5.16.1p1 and 6.3.2.3p3 for scalar assignment and null pointers.
fn assignable(destination: &Type, expr: &Expression<'_>, target: Target) -> bool {
    let converted = expr.converted_type();
    let source = converted.as_ref().unwrap_or(&expr.ty);
    match (&destination.kind, &source.kind) {
        (Kind::Integer(_) | Kind::Float(_), Kind::Integer(_) | Kind::Float(_))
        | (Kind::Integer(Integer::Bool), Kind::Pointer(_)) => true,
        (Kind::Pointer(_), Kind::Integer(_)) => is_null(expr, target),
        (Kind::Pointer(to), Kind::Pointer(from)) => {
            if !to.qualifiers.contains(from.qualifiers) {
                return false;
            }
            to.kind == from.kind
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
fn decode_string(token: &Lexeme, bytes: &mut Vec<u8>) -> Result<(), Diagnostic> {
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
