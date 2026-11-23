//! Recursive descent for N1570 declarations, blocks and call expressions.
use std::collections::HashMap;
use std::iter::Peekable;
use std::path::Path;

use crate::pp::{Diagnostic, Kind, Lexeme, Location, Position, Preprocessed, Standard, Target};
use crate::types::{amd64_va_list, Integer, Kind as TypeKind, Qualifiers, Record, Type};

mod attribute;
pub(crate) use attribute::SymbolAttributes;
mod declaration;
mod specifiers;

pub(super) const MAX_NESTING: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Storage {
    #[default]
    Unspecified,
    Extern,
    Static,
    Typedef,
    Auto,
    Register,
}

#[derive(Clone, Debug, Default)]
struct DeclStorage {
    class: Storage,
    thread_local: bool,
    alignment: Option<usize>,
    gnu_alignment: usize,
    returns_twice: bool,
    noreturn: bool,
    inline: bool,
    anonymous_record: bool,
    may_alias: bool,
    function_hint: Option<&'static str>,
    symbol: SymbolAttributes,
}

#[derive(Debug)]
pub(super) enum Declaration<'a> {
    Function(Function<'a>),
    Object(Variable<'a>),
    StaticAssert(StaticAssertion<'a>),
    Record(Record<'a>),
}

#[derive(Clone, Debug)]
pub(super) struct StaticAssertion<'a> {
    pub site: &'a Lexeme,
    pub expression: Expression<'a>,
    pub message: String,
}

#[derive(Clone, Debug)]
pub(super) struct Variable<'a> {
    pub name: &'a Lexeme,
    pub ty: Type,
    pub storage: Storage,
    pub thread_local: bool,
    pub alignment: Option<usize>,
    pub initializer: Option<Initializer<'a>>,
    pub symbol: SymbolAttributes,
}

#[derive(Clone, Debug)]
pub(super) enum Initializer<'a> {
    Expression(Expression<'a>),
    List(&'a Lexeme, Vec<InitializerEntry<'a>>),
}

#[derive(Clone, Debug)]
pub(super) struct InitializerEntry<'a> {
    pub designators: Vec<Designator<'a>>,
    pub value: Initializer<'a>,
}

#[derive(Clone, Debug)]
pub(super) enum Designator<'a> {
    Index(Expression<'a>),
    Range(Expression<'a>, Expression<'a>),
    Member(&'a Lexeme),
}

#[derive(Debug)]
pub(super) struct Function<'a> {
    pub name: &'a Lexeme,
    pub result: Type,
    pub params: Vec<Parameter<'a>>,
    pub variadic: bool,
    pub prototype: bool,
    pub noreturn: bool,
    pub returns_twice: bool,
    pub storage: Storage,
    pub body: Option<Vec<Statement<'a>>>,
    pub symbol: SymbolAttributes,
}

#[derive(Debug)]
pub(super) struct Parameter<'a> {
    pub start: &'a Lexeme,
    pub name: Option<&'a Lexeme>,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub(super) enum Statement<'a> {
    Block(Vec<Self>),
    Empty,
    Declaration(Vec<Variable<'a>>),
    StaticAssert(StaticAssertion<'a>),
    Expression(Expression<'a>),
    Return(&'a Lexeme, Option<Expression<'a>>),
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
    },
    Switch(Expression<'a>, Box<Self>),
    Case {
        site: &'a Lexeme,
        low: Option<Expression<'a>>,
        high: Option<Expression<'a>>,
        body: Box<Self>,
    },
    Label(&'a Lexeme, Box<Self>),
    Goto(&'a Lexeme),
    Break(&'a Lexeme),
    Continue(&'a Lexeme),
}

#[derive(Clone, Debug)]
pub(super) struct Expression<'a> {
    pub site: &'a Lexeme,
    pub kind: ExpressionKind<'a>,
    height: usize,
}

impl<'a> Expression<'a> {
    fn new(site: &'a Lexeme, kind: ExpressionKind<'a>) -> Result<Self, Diagnostic> {
        use ExpressionKind::*;
        let child_height = match &kind {
            Unary(_, expr)
            | Cast(_, expr)
            | Sizeof(expr)
            | VaArg(expr, _)
            | Update { operand: expr, .. }
            | Member { base: expr, .. } => expr.height,
            Binary(_, a, b) | Assignment(_, a, b) | Comma(a, b) => a.height.max(b.height),
            OmittedConditional(a, b) => a.height.max(b.height),
            Conditional(a, b, c) => a.height.max(b.height).max(c.height),
            Generic(control, associations) => associations
                .iter()
                .fold(control.height, |height, association| {
                    height.max(association.expression.height)
                }),
            Offsetof(_, designators) => designators
                .iter()
                .filter_map(|designator| match designator {
                    Designator::Index(index) => Some(index.height),
                    Designator::Range(low, high) => Some(low.height.max(high.height)),
                    Designator::Member(_) => None,
                })
                .max()
                .unwrap_or(0),
            Call {
                callee, arguments, ..
            } => arguments
                .iter()
                .fold(callee.height, |height, arg| height.max(arg.height)),
            _ => 0,
        };
        // Parentheses disappear from the tree and left-associative operators
        // build a spine. Bound the stored tree as well as parser recursion.
        if child_height >= MAX_NESTING {
            return Err(site.error("expression nesting limit exceeded"));
        }
        Ok(Self {
            site,
            kind,
            height: child_height + 1,
        })
    }
}

#[derive(Clone, Debug)]
pub(super) enum ExpressionKind<'a> {
    Identifier,
    Enumerator(i32),
    Number,
    Character,
    Unary(Unary, Box<Expression<'a>>),
    Binary(Binary, Box<Expression<'a>>, Box<Expression<'a>>),
    Comma(Box<Expression<'a>>, Box<Expression<'a>>),
    Cast(Type, Box<Expression<'a>>),
    CompoundLiteral(Type, Box<Initializer<'a>>),
    StatementExpression(Vec<Statement<'a>>),
    OmittedConditional(Box<Expression<'a>>, Box<Expression<'a>>),
    SizeofType(Type),
    Alignof(Type),
    Generic(Box<Expression<'a>>, Vec<Association<'a>>),
    Offsetof(Type, Vec<Designator<'a>>),
    TypesCompatible(Type, Type),
    Sizeof(Box<Expression<'a>>),
    VaArg(Box<Expression<'a>>, Type),
    FltRounds,
    Conditional(
        Box<Expression<'a>>,
        Box<Expression<'a>>,
        Box<Expression<'a>>,
    ),
    Assignment(Option<Binary>, Box<Expression<'a>>, Box<Expression<'a>>),
    Update {
        operand: Box<Expression<'a>>,
        increment: bool,
        postfix: bool,
    },
    Member {
        base: Box<Expression<'a>>,
        name: &'a Lexeme,
        indirect: bool,
    },
    String(Vec<&'a Lexeme>),
    Call {
        callee: Box<Expression<'a>>,
        arguments: Vec<Expression<'a>>,
        close: &'a Lexeme,
    },
}

#[derive(Clone, Debug)]
pub(super) struct Association<'a> {
    pub site: &'a Lexeme,
    pub ty: Option<Type>,
    pub expression: Expression<'a>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Unary {
    Plus,
    Minus,
    Complement,
    Not,
    Dereference,
    Address,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Binary {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    ShiftLeft,
    ShiftRight,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    BitAnd,
    BitXor,
    BitOr,
    LogicalAnd,
    LogicalOr,
}

impl Binary {
    fn parse(text: &str) -> Option<(u8, Self)> {
        use Binary::*;
        Some(match text {
            "||" => (1, LogicalOr),
            "&&" => (2, LogicalAnd),
            "|" => (3, BitOr),
            "^" => (4, BitXor),
            "&" => (5, BitAnd),
            "==" => (6, Equal),
            "!=" => (6, NotEqual),
            "<" => (7, Less),
            "<=" => (7, LessEqual),
            ">" => (7, Greater),
            ">=" => (7, GreaterEqual),
            "<<" => (8, ShiftLeft),
            ">>" => (8, ShiftRight),
            "+" => (9, Add),
            "-" => (9, Subtract),
            "*" => (10, Multiply),
            "/" => (10, Divide),
            "%" => (10, Remainder),
            _ => return None,
        })
    }
}

pub(super) fn translation_unit<'a>(
    source: &'a Preprocessed,
    path: &Path,
    target: Target,
) -> Result<Vec<Declaration<'a>>, Diagnostic> {
    parse_tokens(source.tokens(), path, target, source.standard)
}

fn parse_tokens<'a>(
    tokens: impl Iterator<Item = &'a Lexeme>,
    path: &Path,
    target: Target,
    standard: Standard,
) -> Result<Vec<Declaration<'a>>, Diagnostic> {
    let mut parser = Parser {
        tokens: tokens.peekable(),
        previous: None,
        path,
        target,
        standard,
        names: vec![HashMap::new()],
        tags: vec![HashMap::new()],
        records: Vec::new(),
        va_list_record: None,
        statement_expressions: 0,
        in_function: false,
    };
    let mut declarations = Vec::new();
    if parser.peek().is_none() {
        return Err(parser.error("expected a declaration"));
    }
    while parser.peek().is_some() {
        declarations.extend(parser.external()?);
    }
    // The type table includes records declared in any scope. Their ids
    // follow allocation order, independent of the object declaration list.
    declarations.extend(parser.records.into_iter().map(Declaration::Record));
    Ok(declarations)
}

struct Parser<'a, 'p, I: Iterator<Item = &'a Lexeme>> {
    tokens: Peekable<I>,
    previous: Option<&'a Lexeme>,
    path: &'p Path,
    target: Target,
    standard: Standard,
    names: Vec<HashMap<&'a str, Name>>,
    tags: Vec<HashMap<&'a str, Tag>>,
    records: Vec<Record<'a>>,
    va_list_record: Option<usize>,
    statement_expressions: usize,
    in_function: bool,
}

#[derive(Clone, Copy)]
enum Tag {
    Enum,
    Record(usize),
}

enum Name {
    Ordinary(Type),
    Typedef(Type, bool),
    Enumerator(i32),
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
        if self
            .peek()
            .is_some_and(|token| is_identifier(token) && !self.is_typeof(token))
        {
            self.take()
        } else {
            None
        }
    }

    fn lookup(&self, name: &str) -> Option<&Name> {
        self.names.iter().rev().find_map(|scope| scope.get(name))
    }

    fn name(&mut self, token: &'a Lexeme, value: Name) {
        self.names.last_mut().unwrap().insert(&token.text, value);
    }

    fn scope<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        self.names.push(HashMap::new());
        self.tags.push(HashMap::new());
        let result = f(self);
        self.tags.pop();
        self.names.pop();
        result
    }

    fn is_typeof(&self, token: &Lexeme) -> bool {
        matches!(token.text.as_ref(), "__typeof" | "__typeof__")
            || self.standard.is_gnu() && token.is("typeof")
    }

    fn starts_type(&self, token: &Lexeme) -> bool {
        self.is_typeof(token)
            || matches!(
                token.text.as_ref(),
                "void"
                    | "_Bool"
                    | "char"
                    | "short"
                    | "int"
                    | "long"
                    | "signed"
                    | "unsigned"
                    | "float"
                    | "double"
                    | "_Complex"
                    | "const"
                    | "volatile"
                    | "restrict"
                    | "__restrict"
                    | "__restrict__"
                    | "_Atomic"
                    | "struct"
                    | "union"
                    | "enum"
                    | amd64_va_list::NAME
            )
            || matches!(self.lookup(&token.text), Some(Name::Typedef(..)))
    }

    fn starts_declaration(&self, token: &Lexeme) -> bool {
        self.starts_type(token)
            || matches!(
                token.text.as_ref(),
                "extern"
                    | "static"
                    | "typedef"
                    | "auto"
                    | "register"
                    | "inline"
                    | "__inline"
                    | "__inline__"
                    | "__attribute__"
                    | "__attribute"
                    | "_Thread_local"
                    | "_Alignas"
                    | "_Noreturn"
            )
    }

    // N1570 6.8.2: a block has its own identifier scope.
    fn block(&mut self, depth: usize) -> Result<Vec<Statement<'a>>, Diagnostic> {
        self.scope(|parser| parser.block_items(depth))
    }

    fn block_items(&mut self, depth: usize) -> Result<Vec<Statement<'a>>, Diagnostic> {
        let mut statements = Vec::new();
        while self.eat("}").is_none() {
            let extension = self.extension_markers()?;
            if let Some(site) = self.eat("_Static_assert") {
                statements.push(Statement::StaticAssert(self.static_assertion(site, depth)?));
            } else if self
                .peek()
                .is_some_and(|token| self.starts_declaration(token))
            {
                statements.push(Statement::Declaration(self.declaration()?));
            } else if extension {
                let expression = self.expression(0)?;
                self.expect(";", "expected ';' after the extension expression")?;
                statements.push(Statement::Expression(expression));
            } else {
                statements.push(self.statement(depth)?);
            }
        }
        Ok(statements)
    }

    fn extension_markers(&mut self) -> Result<bool, Diagnostic> {
        for count in 0..MAX_NESTING {
            if self.eat("__extension__").is_none() {
                return Ok(count != 0);
            }
        }
        Err(self.error("extension marker nesting limit exceeded"))
    }

    fn statement(&mut self, depth: usize) -> Result<Statement<'a>, Diagnostic> {
        if depth >= MAX_NESTING {
            return Err(self.error("statement nesting limit exceeded"));
        }
        if self.eat("{").is_some() {
            return Ok(Statement::Block(self.block(depth + 1)?));
        }
        if self.eat(";").is_some() {
            return Ok(Statement::Empty);
        }
        if let Some(keyword) = self.eat("return") {
            return Ok(Statement::Return(keyword, self.optional_expression(";")?));
        }
        if self.eat("if").is_some() {
            let condition = self.parenthesized()?;
            let yes = Box::new(self.statement(depth + 1)?);
            let no = if self.eat("else").is_some() {
                Some(Box::new(self.statement(depth + 1)?))
            } else {
                None
            };
            return Ok(Statement::If { condition, yes, no });
        }
        if self.eat("for").is_some() {
            return self.scope(|parser| parser.for_loop(depth));
        }
        if self.eat("while").is_some() {
            let condition = Some(self.parenthesized()?);
            let body = Box::new(self.statement(depth + 1)?);
            return Ok(Statement::Loop {
                condition,
                post_test: false,
                increment: None,
                body,
            });
        }
        if self.eat("do").is_some() {
            let body = Box::new(self.statement(depth + 1)?);
            self.expect("while", "expected 'while' after the do body")?;
            let condition = Some(self.parenthesized()?);
            self.expect(";", "expected ';' after the do condition")?;
            return Ok(Statement::Loop {
                condition,
                post_test: true,
                increment: None,
                body,
            });
        }
        if self.eat("switch").is_some() {
            let value = self.parenthesized()?;
            return Ok(Statement::Switch(
                value,
                Box::new(self.statement(depth + 1)?),
            ));
        }
        if let Some(site) = self.eat("case").or_else(|| self.eat("default")) {
            let low = if site.is("case") {
                Some(self.conditional(0)?)
            } else {
                None
            };
            let high = if low.is_some() && self.eat("...").is_some() {
                Some(self.conditional(0)?)
            } else {
                None
            };
            self.expect(":", "expected ':' after the case label")?;
            return Ok(Statement::Case {
                site,
                low,
                high,
                body: Box::new(self.statement(depth + 1)?),
            });
        }
        if self.eat("goto").is_some() {
            let label = self
                .identifier()
                .ok_or_else(|| self.error("expected a label name"))?;
            self.expect(";", "expected ';' after the label name")?;
            return Ok(Statement::Goto(label));
        }
        if let Some(site) = self.eat("break").or_else(|| self.eat("continue")) {
            self.expect(";", "expected ';' after the jump statement")?;
            return Ok(if site.is("break") {
                Statement::Break(site)
            } else {
                Statement::Continue(site)
            });
        }
        let expr = self.expression(0)?;
        if matches!(
            expr.kind,
            ExpressionKind::Identifier | ExpressionKind::Enumerator(_)
        ) && self.eat(":").is_some()
        {
            return Ok(Statement::Label(
                expr.site,
                Box::new(self.statement(depth + 1)?),
            ));
        }
        self.expect(";", "expected ';' after the expression")?;
        Ok(Statement::Expression(expr))
    }

    fn for_loop(&mut self, depth: usize) -> Result<Statement<'a>, Diagnostic> {
        self.expect("(", "expected '(' after 'for'")?;
        let extension = self.extension_markers()?;
        let initial = if self
            .peek()
            .is_some_and(|token| self.starts_declaration(token))
        {
            let variables = self.declaration()?;
            if let Some(variable) = variables.iter().find(|variable| variable.thread_local) {
                return Err(variable
                    .name
                    .error("a for initializer cannot declare a _Thread_local object"));
            }
            Statement::Declaration(variables)
        } else if extension {
            let expression = self.expression(0)?;
            self.expect(";", "expected ';' after the extension expression")?;
            Statement::Expression(expression)
        } else {
            self.optional_expression(";")?
                .map_or(Statement::Empty, Statement::Expression)
        };
        let condition = self.optional_expression(";")?;
        let increment = self.optional_expression(")")?;
        let body = Box::new(self.statement(depth + 1)?);
        Ok(Statement::Block(vec![
            initial,
            Statement::Loop {
                condition,
                post_test: false,
                increment,
                body,
            },
        ]))
    }

    fn parenthesized(&mut self) -> Result<Expression<'a>, Diagnostic> {
        self.expect("(", "expected '(' before the condition")?;
        let expr = self.expression(0)?;
        self.expect(")", "expected ')' after the condition")?;
        Ok(expr)
    }

    fn optional_expression(&mut self, end: &str) -> Result<Option<Expression<'a>>, Diagnostic> {
        if self.eat(end).is_some() {
            return Ok(None);
        }
        let expr = self.expression(0)?;
        self.expect(
            end,
            if end == ";" {
                "expected ';' after the expression"
            } else {
                "expected ')' after the expression"
            },
        )?;
        Ok(Some(expr))
    }

    fn expression(&mut self, depth: usize) -> Result<Expression<'a>, Diagnostic> {
        // N1570 6.5.17: comma sequences assignment expressions. A comma
        // separating arguments, initializers or associations is not an operator.
        let mut left = self.assignment(depth)?;
        while let Some(comma) = self.eat(",") {
            let right = self.assignment(depth)?;
            left = Expression::new(comma, ExpressionKind::Comma(Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }

    fn assignment(&mut self, depth: usize) -> Result<Expression<'a>, Diagnostic> {
        let left = self.conditional(depth)?;
        let Some(token) = self.peek() else {
            return Ok(left);
        };
        let op = match token.text.as_ref() {
            "=" => None,
            "+=" => Some(Binary::Add),
            "-=" => Some(Binary::Subtract),
            "*=" => Some(Binary::Multiply),
            "/=" => Some(Binary::Divide),
            "%=" => Some(Binary::Remainder),
            "&=" => Some(Binary::BitAnd),
            "^=" => Some(Binary::BitXor),
            "|=" => Some(Binary::BitOr),
            "<<=" => Some(Binary::ShiftLeft),
            ">>=" => Some(Binary::ShiftRight),
            _ => return Ok(left),
        };
        self.take();
        let right = self.assignment(depth + 1)?;
        Expression::new(
            token,
            ExpressionKind::Assignment(op, Box::new(left), Box::new(right)),
        )
    }

    fn conditional(&mut self, depth: usize) -> Result<Expression<'a>, Diagnostic> {
        let test = self.binary(1, depth)?;
        let Some(site) = self.eat("?") else {
            return Ok(test);
        };
        if self.eat(":").is_some() {
            let no = self.conditional(depth + 1)?;
            return Expression::new(
                site,
                ExpressionKind::OmittedConditional(Box::new(test), Box::new(no)),
            );
        }
        let yes = self.expression(depth + 1)?;
        self.expect(":", "expected ':' in the conditional expression")?;
        let no = self.conditional(depth + 1)?;
        Expression::new(
            site,
            ExpressionKind::Conditional(Box::new(test), Box::new(yes), Box::new(no)),
        )
    }

    // N1570 6.5.5–6.5.14: higher precedence binds tighter. The right side
    // excludes the current precedence so operators associate to the left.
    fn binary(&mut self, minimum: u8, depth: usize) -> Result<Expression<'a>, Diagnostic> {
        let mut left = self.unary(depth)?;
        let mut count = 0;
        while let Some((precedence, op)) = self.peek().and_then(|t| Binary::parse(&t.text)) {
            if precedence < minimum {
                break;
            }
            let site = self.take().unwrap();
            if depth + count == MAX_NESTING {
                return Err(site.error("expression nesting limit exceeded"));
            }
            count += 1;
            let right = self.binary(precedence + 1, depth + count)?;
            left = Expression::new(
                site,
                ExpressionKind::Binary(op, Box::new(left), Box::new(right)),
            )?;
        }
        Ok(left)
    }

    fn unary(&mut self, depth: usize) -> Result<Expression<'a>, Diagnostic> {
        if depth >= MAX_NESTING {
            return Err(self.error("expression nesting limit exceeded"));
        }
        if self.eat("__extension__").is_some() {
            return self.unary(depth + 1);
        }
        if let Some(site) = self.eat("++").or_else(|| self.eat("--")) {
            let operand = Box::new(self.unary(depth + 1)?);
            return Expression::new(
                site,
                ExpressionKind::Update {
                    operand,
                    increment: site.is("++"),
                    postfix: false,
                },
            );
        }
        if let Some(site) = self.eat("_Alignof") {
            self.expect("(", "expected '(' after _Alignof")?;
            let ty = self.type_name(depth + 1)?;
            self.expect(")", "expected ')' after the _Alignof type")?;
            return Expression::new(site, ExpressionKind::Alignof(ty));
        }
        if let Some(site) = self.eat("sizeof") {
            let kind = if let Some(open) = self.eat("(") {
                if self.eat("{").is_some() {
                    let expression = self.statement_expression(open, depth + 1)?;
                    ExpressionKind::Sizeof(Box::new(self.postfix(expression, depth + 1)?))
                } else if self.peek().is_some_and(|token| self.starts_type(token)) {
                    let ty = self.type_name(depth + 1)?;
                    self.expect(")", "expected ')' after the sizeof type")?;
                    if let Some(open) = self.eat("{") {
                        let literal = self.compound_literal(open, ty, depth + 1)?;
                        ExpressionKind::Sizeof(Box::new(self.postfix(literal, depth + 1)?))
                    } else {
                        ExpressionKind::SizeofType(ty)
                    }
                } else {
                    let expression = self.expression(depth + 1)?;
                    self.expect(")", "expected ')' after the sizeof operand")?;
                    ExpressionKind::Sizeof(Box::new(expression))
                }
            } else {
                ExpressionKind::Sizeof(Box::new(self.unary(depth + 1)?))
            };
            return Expression::new(site, kind);
        }
        if let Some((site, op)) = self.peek().and_then(|site| {
            let op = match site.text.as_ref() {
                "+" => Unary::Plus,
                "-" => Unary::Minus,
                "~" => Unary::Complement,
                "!" => Unary::Not,
                "*" => Unary::Dereference,
                "&" => Unary::Address,
                _ => return None,
            };
            Some((site, op))
        }) {
            self.take();
            return Expression::new(
                site,
                ExpressionKind::Unary(op, Box::new(self.unary(depth + 1)?)),
            );
        }
        let callee = if let Some(site) = self.eat("(") {
            if self.eat("{").is_some() {
                self.statement_expression(site, depth + 1)?
            } else if self.peek().is_some_and(|token| self.starts_type(token)) {
                let ty = self.type_name(depth + 1)?;
                self.expect(")", "expected ')' after the cast type")?;
                if let Some(open) = self.eat("{") {
                    self.compound_literal(open, ty, depth + 1)?
                } else {
                    return Expression::new(
                        site,
                        ExpressionKind::Cast(ty, Box::new(self.unary(depth + 1)?)),
                    );
                }
            } else {
                let expr = self.expression(depth + 1)?;
                self.expect(")", "expected ')' after the expression")?;
                expr
            }
        } else {
            self.primary(depth)?
        };
        self.postfix(callee, depth)
    }

    fn statement_expression(
        &mut self,
        site: &'a Lexeme,
        depth: usize,
    ) -> Result<Expression<'a>, Diagnostic> {
        if !self.in_function {
            return Err(site.error("statement expression is only allowed inside a function"));
        }
        if depth >= MAX_NESTING || self.statement_expressions >= MAX_NESTING {
            return Err(site.error("statement expression nesting limit exceeded"));
        }
        self.statement_expressions += 1;
        let statements = self.block(depth);
        self.statement_expressions -= 1;
        let statements = statements?;
        self.expect(")", "expected ')' after the statement expression")?;
        Expression::new(site, ExpressionKind::StatementExpression(statements))
    }

    fn compound_literal(
        &mut self,
        open: &'a Lexeme,
        ty: Type,
        depth: usize,
    ) -> Result<Expression<'a>, Diagnostic> {
        let init = self.initializer_list(open, depth)?;
        Expression::new(open, ExpressionKind::CompoundLiteral(ty, Box::new(init)))
    }

    fn postfix(
        &mut self,
        mut callee: Expression<'a>,
        depth: usize,
    ) -> Result<Expression<'a>, Diagnostic> {
        let mut count = 0;
        while let Some(open) = self
            .eat("(")
            .or_else(|| self.eat("["))
            .or_else(|| self.eat("."))
            .or_else(|| self.eat("->"))
            .or_else(|| self.eat("++"))
            .or_else(|| self.eat("--"))
        {
            if depth + count == MAX_NESTING {
                return Err(open.error("expression nesting limit exceeded"));
            }
            if open.is("++") || open.is("--") {
                callee = Expression::new(
                    open,
                    ExpressionKind::Update {
                        operand: Box::new(callee),
                        increment: open.is("++"),
                        postfix: true,
                    },
                )?;
                count += 1;
                continue;
            }
            if open.is("[") {
                let right = self.expression(depth + count + 1)?;
                self.expect("]", "expected ']' after the subscript")?;
                let sum = Expression::new(
                    open,
                    ExpressionKind::Binary(Binary::Add, Box::new(callee), Box::new(right)),
                )?;
                callee = Expression::new(
                    open,
                    ExpressionKind::Unary(Unary::Dereference, Box::new(sum)),
                )?;
                count += 1;
                continue;
            }
            count += 1;
            if open.is(".") || open.is("->") {
                let name = self
                    .identifier()
                    .ok_or_else(|| self.error("expected a member name"))?;
                callee = Expression::new(
                    open,
                    ExpressionKind::Member {
                        base: Box::new(callee),
                        name,
                        indirect: open.is("->"),
                    },
                )?;
                continue;
            }
            let mut arguments = Vec::new();
            let close = if let Some(close) = self.eat(")") {
                close
            } else {
                loop {
                    arguments.push(self.assignment(depth + count)?);
                    if self.eat(",").is_none() {
                        break self.expect(")", "expected ',' or ')' after the argument")?;
                    }
                }
            };
            callee = Expression::new(
                callee.site,
                ExpressionKind::Call {
                    callee: Box::new(callee),
                    arguments,
                    close,
                },
            )?;
        }
        Ok(callee)
    }

    fn primary(&mut self, depth: usize) -> Result<Expression<'a>, Diagnostic> {
        let site = self
            .peek()
            .ok_or_else(|| self.error("expected an expression"))?;
        let kind = match site.kind {
            Kind::Identifier if site.is("_Generic") => {
                self.take();
                self.expect("(", "expected '(' after _Generic")?;
                let control = self.assignment(depth + 1)?;
                self.expect(",", "expected ',' before generic associations")?;
                let mut associations = Vec::new();
                loop {
                    let site = self
                        .peek()
                        .ok_or_else(|| self.error("expected a generic association"))?;
                    let ty = if self.eat("default").is_some() {
                        None
                    } else {
                        Some(self.type_name(depth + 1)?)
                    };
                    self.expect(":", "expected ':' in the generic association")?;
                    associations.push(Association {
                        site,
                        ty,
                        expression: self.assignment(depth + 1)?,
                    });
                    if self.eat(",").is_none() {
                        break;
                    }
                }
                self.expect(")", "expected ')' after generic associations")?;
                ExpressionKind::Generic(Box::new(control), associations)
            }
            Kind::Identifier if site.is("__builtin_types_compatible_p") => {
                self.take();
                self.expect("(", "expected '(' after __builtin_types_compatible_p")?;
                let left = self.type_name(depth + 1)?;
                self.expect(",", "expected ',' between the type names")?;
                let right = self.type_name(depth + 1)?;
                self.expect(")", "expected ')' after the type names")?;
                ExpressionKind::TypesCompatible(left, right)
            }
            Kind::Identifier if site.is("__builtin_offsetof") => {
                self.take();
                self.expect("(", "expected '(' after __builtin_offsetof")?;
                let ty = self.type_name(depth + 1)?;
                self.expect(",", "expected ',' before the member designator")?;
                let field = self
                    .identifier()
                    .ok_or_else(|| self.error("expected a member name"))?;
                let mut path = vec![Designator::Member(field)];
                self.designators(&mut path, site, depth + 1, false)?;
                self.expect(")", "expected ')' after the member designator")?;
                ExpressionKind::Offsetof(ty, path)
            }
            Kind::Identifier if site.is("__builtin_va_arg") => {
                self.take();
                self.expect("(", "expected '(' after __builtin_va_arg")?;
                let list = self.assignment(depth + 1)?;
                self.expect(",", "expected ',' before the va_arg type")?;
                let ty = self.type_name(depth + 1)?;
                self.expect(")", "expected ')' after the va_arg type")?;
                ExpressionKind::VaArg(Box::new(list), ty)
            }
            Kind::Identifier if site.is("__builtin_flt_rounds") => {
                self.take();
                self.expect("(", "expected '(' after __builtin_flt_rounds")?;
                self.expect(")", "__builtin_flt_rounds takes no arguments")?;
                ExpressionKind::FltRounds
            }
            Kind::Identifier if is_identifier(site) && !self.is_typeof(site) => {
                self.take();
                match self.lookup(&site.text) {
                    Some(Name::Enumerator(value)) => ExpressionKind::Enumerator(*value),
                    _ => ExpressionKind::Identifier,
                }
            }
            Kind::Number => {
                self.take();
                ExpressionKind::Number
            }
            Kind::Character => {
                self.take();
                ExpressionKind::Character
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
        Expression::new(site, kind)
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
                | "__inline"
                | "__inline__"
                | "__extension__"
                | "__attribute__"
                | "int"
                | "long"
                | "register"
                | "restrict"
                | "__restrict"
                | "__restrict__"
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
                | amd64_va_list::NAME
                | "__builtin_va_arg"
                | "__builtin_offsetof"
                | "__builtin_types_compatible_p"
                | "__builtin_flt_rounds"
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
            let parsed = parse_tokens(
                source.tokens().take(count),
                path,
                Target::X86_64,
                source.standard,
            );
            if count == 9 {
                // A complete prototype on its own is a valid translation unit.
                assert_eq!(parsed.unwrap().len(), 1);
            } else {
                let error = parsed.unwrap_err().to_string();
                assert!(error.starts_with("truncated.c:1:"), "{error}");
                assert!(error.contains("unexpected end of input"), "{error}");
            }
        }
        assert_eq!(
            translation_unit(&source, path, Target::X86_64)
                .unwrap()
                .len(),
            2
        );
    }
}
