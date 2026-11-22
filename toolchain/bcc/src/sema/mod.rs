//! Typed translation units for the hello subset of N1570.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::parse::{self, Storage};
use crate::pp::{self, Diagnostic, Lexeme, Position, Preprocessed, Target};
use crate::types::{Integer, Kind, Type};

mod dump;

#[derive(Debug)]
pub struct TranslationUnit<'a> {
    pub functions: Vec<Function<'a>>,
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
}

#[derive(Debug)]
pub struct Parameter<'a> {
    pub name: Option<(&'a str, &'a Position)>,
    pub ty: Type,
}

#[derive(Debug)]
pub enum Statement<'a> {
    Block(Vec<Self>),
    Empty,
    Expression(Expression<'a>),
    Return(Option<Expression<'a>>),
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
    Integer(u64),
    String(Vec<u8>),
    Call {
        callee: Box<Expression<'a>>,
        arguments: Vec<Argument<'a>>,
    },
}

#[derive(Debug)]
pub struct Argument<'a> {
    pub expr: Expression<'a>,
    /// Type after the conversion to the prototype's parameter type.
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
            _ if matches!(self.kind, ExpressionKind::Identifier(_))
                && self.ty.qualifiers != Default::default() =>
            {
                Some(self.ty.unqualified())
            }
            _ => None,
        }
    }
}

pub fn analyze<'a>(
    source: &'a Preprocessed,
    path: &Path,
    target: Target,
) -> Result<TranslationUnit<'a>, Diagnostic> {
    let mut symbols = HashMap::new();
    let mut functions = Vec::new();
    for declaration in parse::translation_unit(source, path)? {
        functions.push(function(declaration, &mut symbols, target)?);
    }
    Ok(TranslationUnit { functions })
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
        storage,
        body,
    } = declaration;
    let defined = body.is_some();
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
    let context = FunctionBody {
        symbols,
        params: &params,
        result: &result,
        target,
    };
    let body = body
        .map(|statements| context.statements(statements))
        .transpose()?;
    Ok(Function {
        name: &name.text,
        site: &name.site,
        ty,
        linkage,
        params,
        body,
    })
}

struct FunctionBody<'a, 's> {
    symbols: &'s HashMap<&'a str, Symbol>,
    params: &'s [Parameter<'a>],
    result: &'s Type,
    target: Target,
}

impl<'a> FunctionBody<'a, '_> {
    fn statements(
        &self,
        statements: Vec<parse::Statement<'a>>,
    ) -> Result<Vec<Statement<'a>>, Diagnostic> {
        statements
            .into_iter()
            .map(|statement| {
                let statement = match statement {
                    parse::Statement::Block(inner) => Statement::Block(self.statements(inner)?),
                    parse::Statement::Empty => Statement::Empty,
                    parse::Statement::Expression(expr) => {
                        Statement::Expression(self.expression(expr)?)
                    }
                    parse::Statement::Return(keyword, value) => {
                        Statement::Return(self.return_value(keyword, value)?)
                    }
                };
                Ok(statement)
            })
            .collect()
    }

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
                if !assignable(self.result, &expr) {
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
                let local = self
                    .params
                    .iter()
                    .find(|param| param.name.is_some_and(|(param_name, _)| name == param_name));
                let ty = local
                    .map(|param| &param.ty)
                    .or_else(|| self.symbols.get(name).map(|symbol| &symbol.ty))
                    .ok_or_else(|| site.error(format!("undeclared identifier '{name}'")))?;
                (ExpressionKind::Identifier(name), ty.clone())
            }
            Syntax::Integer => {
                let (value, integer) = integer_literal(site, self.target)?;
                (
                    ExpressionKind::Integer(value),
                    Type::new(Kind::Integer(integer)),
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
        let Kind::Function { result, params } = &function.kind else {
            return Err(callee.site.error("called expression is not a function"));
        };

        // N1570 6.5.2.2p2, p5 and p7: check the prototype and convert each
        // argument as if assigned to its unqualified parameter type.
        let mut converted_arguments = Vec::new();
        for (index, argument) in arguments.into_iter().enumerate() {
            let parameter = params
                .get(index)
                .ok_or_else(|| argument.site.error("too many arguments"))?;
            let expr = self.expression(argument)?;
            if !assignable(parameter, &expr) {
                return Err(expr
                    .site
                    .error(format!("cannot pass {} as {parameter}", expr.ty)));
            }
            converted_arguments.push(Argument {
                expr,
                ty: parameter.clone(),
            });
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

// N1570 6.5.16.1p1 and 6.3.2.3p3 for scalar assignment and null pointers.
fn assignable(destination: &Type, expr: &Expression<'_>) -> bool {
    let converted = expr.converted_type();
    let source = converted.as_ref().unwrap_or(&expr.ty);
    match (&destination.kind, &source.kind) {
        (Kind::Integer(_), Kind::Integer(_)) | (Kind::Integer(Integer::Bool), Kind::Pointer(_)) => {
            true
        }
        (Kind::Pointer(_), Kind::Integer(_)) => matches!(expr.kind, ExpressionKind::Integer(0)),
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
