//! Recursive descent for N1570 declarations, blocks and call expressions.
use std::iter::Peekable;
use std::path::Path;

use crate::pp::{Diagnostic, Kind, Lexeme, Location, Position, Preprocessed};
use crate::types::{Integer, Kind as TypeKind, Qualifiers, Type};

mod specifiers;

const MAX_NESTING: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Storage {
    #[default]
    Unspecified,
    Extern,
    Static,
}

#[derive(Debug)]
pub(super) struct Function<'a> {
    pub name: &'a Lexeme,
    pub result: Type,
    pub params: Vec<Parameter<'a>>,
    pub storage: Storage,
    pub body: Option<Vec<Statement<'a>>>,
}

#[derive(Debug)]
pub(super) struct Parameter<'a> {
    pub start: &'a Lexeme,
    pub name: Option<&'a Lexeme>,
    pub ty: Type,
}

#[derive(Debug)]
pub(super) enum Statement<'a> {
    Block(Vec<Self>),
    Empty,
    Expression(Expression<'a>),
    Return(&'a Lexeme, Option<Expression<'a>>),
}

#[derive(Debug)]
pub(super) struct Expression<'a> {
    pub site: &'a Lexeme,
    pub kind: ExpressionKind<'a>,
}

#[derive(Debug)]
pub(super) enum ExpressionKind<'a> {
    Identifier,
    Integer,
    String(Vec<&'a Lexeme>),
    Call {
        callee: Box<Expression<'a>>,
        arguments: Vec<Expression<'a>>,
        close: &'a Lexeme,
    },
}

pub(super) fn translation_unit<'a>(
    source: &'a Preprocessed,
    path: &Path,
) -> Result<Vec<Function<'a>>, Diagnostic> {
    parse_tokens(source.tokens(), path)
}

fn parse_tokens<'a>(
    tokens: impl Iterator<Item = &'a Lexeme>,
    path: &Path,
) -> Result<Vec<Function<'a>>, Diagnostic> {
    let mut parser = Parser {
        tokens: tokens.peekable(),
        previous: None,
        path,
    };
    let mut functions = vec![parser.function()?];
    while parser.peek().is_some() {
        functions.push(parser.function()?);
    }
    Ok(functions)
}

struct Parser<'a, 'p, I: Iterator<Item = &'a Lexeme>> {
    tokens: Peekable<I>,
    previous: Option<&'a Lexeme>,
    path: &'p Path,
}

impl<'a, I: Iterator<Item = &'a Lexeme>> Parser<'a, '_, I> {
    fn peek(&mut self) -> Option<&'a Lexeme> {
        self.tokens.peek().copied()
    }

    fn take(&mut self) -> Option<&'a Lexeme> {
        let token = self.tokens.next()?;
        self.previous = Some(token);
        Some(token)
    }

    fn eat(&mut self, spelling: &str) -> Option<&'a Lexeme> {
        if self.peek().is_some_and(|token| {
            token.is(spelling)
                || matches!((spelling, token.text.as_ref()), ("{", "<%") | ("}", "%>"))
        }) {
            self.take()
        } else {
            None
        }
    }

    fn expect(&mut self, spelling: &str, message: &str) -> Result<&'a Lexeme, Diagnostic> {
        self.eat(spelling).ok_or_else(|| self.error(message))
    }

    fn error(&mut self, message: &str) -> Diagnostic {
        match self.peek() {
            Some(token) if token.kind == Kind::Other => {
                token.error(format!("invalid token {}", token.text))
            }
            Some(token) => token.error(message),
            None => {
                let message = format!("unexpected end of input: {message}");
                if let Some(previous) = self.previous {
                    previous.error(message)
                } else {
                    Diagnostic::new(
                        &Position::new(self.path, Location { line: 1, column: 1 }),
                        message,
                    )
                }
            }
        }
    }

    fn identifier(&mut self) -> Option<&'a Lexeme> {
        if self.peek().is_some_and(is_identifier) {
            self.take()
        } else {
            None
        }
    }

    // N1570 6.7.6 and 6.9.1. The direct declarator in this subset is a
    // named function with a prototype and scalar or pointer parameters.
    fn function(&mut self) -> Result<Function<'a>, Diagnostic> {
        let (result, storage) = self.specifiers(true)?;
        let result = self.pointers(result)?;
        let name = self
            .identifier()
            .ok_or_else(|| self.error("expected a function name"))?;
        self.expect("(", "expected '(' after the function name")?;
        let params = self.parameters()?;
        let body = if self.eat(";").is_some() {
            None
        } else {
            self.expect("{", "expected ';' or a function body")?;
            Some(self.block(0)?)
        };
        Ok(Function {
            name,
            result,
            params,
            storage,
            body,
        })
    }

    fn parameters(&mut self) -> Result<Vec<Parameter<'a>>, Diagnostic> {
        let mut params = Vec::new();
        loop {
            let start = self
                .peek()
                .ok_or_else(|| self.error("expected a parameter type"))?;
            let (ty, _) = self.specifiers(false)?;
            let ty = self.pointers(ty)?;
            let name = self.identifier();
            // N1570 6.7.6.3p10: a sole unnamed void parameter is an empty list.
            if ty.kind == TypeKind::Void {
                if params.is_empty() && name.is_none() && ty.qualifiers == Qualifiers::default() {
                    self.expect(")", "expected ')' after 'void'")?;
                    return Ok(params);
                }
                return Err(start.error("parameter has void type"));
            }
            params.push(Parameter { start, name, ty });
            if self.eat(",").is_none() {
                self.expect(")", "expected ',' or ')' after the parameter")?;
                return Ok(params);
            }
        }
    }

    fn pointers(&mut self, mut ty: Type) -> Result<Type, Diagnostic> {
        let mut depth = 0;
        while let Some(star) = self.eat("*") {
            if depth == MAX_NESTING {
                return Err(star.error("pointer declarator nesting limit exceeded"));
            }
            ty = ty.pointer_to();
            while self.qualifier(&mut ty.qualifiers) {}
            depth += 1;
        }
        Ok(ty)
    }

    // N1570 6.8.2, 6.8.3 and 6.8.6.4.
    fn block(&mut self, depth: usize) -> Result<Vec<Statement<'a>>, Diagnostic> {
        if depth == MAX_NESTING {
            return Err(self.error("compound statement nesting limit exceeded"));
        }
        let mut statements = Vec::new();
        while self.eat("}").is_none() {
            let statement = if self.eat("{").is_some() {
                Statement::Block(self.block(depth + 1)?)
            } else if self.eat(";").is_some() {
                Statement::Empty
            } else if let Some(keyword) = self.eat("return") {
                let expr = if self.eat(";").is_some() {
                    None
                } else {
                    let expr = self.expression(0)?;
                    self.expect(";", "expected ';' after the return expression")?;
                    Some(expr)
                };
                Statement::Return(keyword, expr)
            } else {
                let expr = self.expression(0)?;
                self.expect(";", "expected ';' after the expression")?;
                Statement::Expression(expr)
            };
            statements.push(statement);
        }
        Ok(statements)
    }

    // N1570 6.5.1 and 6.5.2.2. In this subset a call cannot return a
    // function pointer, so no call can immediately serve as another callee.
    fn expression(&mut self, depth: usize) -> Result<Expression<'a>, Diagnostic> {
        if depth == MAX_NESTING {
            return Err(self.error("expression nesting limit exceeded"));
        }
        let callee = self.primary(depth)?;
        if self.eat("(").is_none() {
            return Ok(callee);
        }
        let mut arguments = Vec::new();
        let close = if let Some(close) = self.eat(")") {
            close
        } else {
            loop {
                arguments.push(self.expression(depth + 1)?);
                if self.eat(",").is_none() {
                    break self.expect(")", "expected ',' or ')' after the argument")?;
                }
            }
        };
        Ok(Expression {
            site: callee.site,
            kind: ExpressionKind::Call {
                callee: Box::new(callee),
                arguments,
                close,
            },
        })
    }

    fn primary(&mut self, depth: usize) -> Result<Expression<'a>, Diagnostic> {
        if self.eat("(").is_some() {
            let expr = self.expression(depth + 1)?;
            self.expect(")", "expected ')' after the expression")?;
            return Ok(expr);
        }
        let site = self
            .peek()
            .ok_or_else(|| self.error("expected an expression"))?;
        let kind = match site.kind {
            Kind::Identifier if is_identifier(site) => {
                self.take();
                ExpressionKind::Identifier
            }
            Kind::Number => {
                self.take();
                ExpressionKind::Integer
            }
            Kind::String => {
                self.take();
                let mut strings = vec![site];
                while let Some(string) = self.peek().filter(|token| token.kind == Kind::String) {
                    self.take();
                    strings.push(string);
                }
                ExpressionKind::String(strings)
            }
            _ => return Err(self.error("expected an identifier, integer or string expression")),
        };
        Ok(Expression { site, kind })
    }
}

fn is_identifier(token: &Lexeme) -> bool {
    // N1570 6.4.1: keywords cannot serve as identifiers, even when their
    // associated grammar is outside the hello subset.
    token.kind == Kind::Identifier
        && !matches!(
            token.text.as_ref(),
            "auto"
                | "break"
                | "case"
                | "char"
                | "const"
                | "continue"
                | "default"
                | "do"
                | "double"
                | "else"
                | "enum"
                | "extern"
                | "float"
                | "for"
                | "goto"
                | "if"
                | "inline"
                | "int"
                | "long"
                | "register"
                | "restrict"
                | "return"
                | "short"
                | "signed"
                | "sizeof"
                | "static"
                | "struct"
                | "switch"
                | "typedef"
                | "union"
                | "unsigned"
                | "void"
                | "volatile"
                | "while"
                | "_Alignas"
                | "_Alignof"
                | "_Atomic"
                | "_Bool"
                | "_Complex"
                | "_Generic"
                | "_Imaginary"
                | "_Noreturn"
                | "_Static_assert"
                | "_Thread_local"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pp::{preprocess_source, Options};

    #[test]
    fn parse_is_error_not_panic() {
        let path = Path::new("truncated.c");
        let source = preprocess_source(
            path,
            b"int puts(const char *s); int main(void) { puts(\"hello\"); return 0; }\n",
            &Options::default(),
        )
        .unwrap();
        let token_count = source.tokens().count();
        for count in 0..token_count {
            let parsed = parse_tokens(source.tokens().take(count), path);
            if count == 9 {
                // A complete prototype on its own is a valid translation unit.
                assert_eq!(parsed.unwrap().len(), 1);
            } else {
                let error = parsed.unwrap_err().to_string();
                assert!(error.starts_with("truncated.c:1:"), "{error}");
                assert!(error.contains("unexpected end of input"), "{error}");
            }
        }
        assert_eq!(translation_unit(&source, path).unwrap().len(), 2);
    }
}
