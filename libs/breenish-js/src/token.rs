//! Token types for the breenish-js lexer.

use alloc::string::String;
use core::fmt;

/// Source position tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub line: u32,
    pub column: u32,
}

impl Span {
    pub fn new(start: u32, end: u32, line: u32, column: u32) -> Self {
        Self { start, end, line, column }
    }
}

/// A token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// All token types recognized by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    Number(f64),
    String(String),
    Identifier(String),

    // Keywords
    Let,
    Const,
    Var,
    Function,
    Return,
    If,
    Else,
    While,
    For,
    Do,
    Break,
    Continue,
    Switch,
    Case,
    Default,
    New,
    Delete,
    Typeof,
    Void,
    In,
    Of,
    Instanceof,
    This,
    Throw,
    Try,
    Catch,
    Finally,
    Async,
    Await,
    Yield,
    Import,
    Export,
    From,
    Class,
    Extends,
    Super,

    // Boolean/Null/Undefined literals
    True,
    False,
    Null,
    Undefined,

    // Arithmetic operators
    Plus,         // +
    Minus,        // -
    Star,         // *
    Slash,        // /
    Percent,      // %
    StarStar,     // **

    // Assignment operators
    Assign,       // =
    PlusAssign,   // +=
    MinusAssign,  // -=
    StarAssign,   // *=
    SlashAssign,  // /=
    PercentAssign, // %=

    // Comparison operators
    Equal,        // ==
    NotEqual,     // !=
    StrictEqual,  // ===
    StrictNotEqual, // !==
    LessThan,     // <
    GreaterThan,  // >
    LessEqual,    // <=
    GreaterEqual, // >=

    // Logical operators
    And,          // &&
    Or,           // ||
    Not,          // !
    NullishCoalesce, // ??

    // Bitwise operators
    BitAnd,       // &
    BitOr,        // |
    BitXor,       // ^
    BitNot,       // ~
    ShiftLeft,    // <<
    ShiftRight,   // >>
    UShiftRight,  // >>>

    // Increment/Decrement
    PlusPlus,     // ++
    MinusMinus,   // --

    // Punctuation
    LeftParen,    // (
    RightParen,   // )
    LeftBrace,    // {
    RightBrace,   // }
    LeftBracket,  // [
    RightBracket, // ]
    Semicolon,    // ;
    Comma,        // ,
    Dot,          // .
    Colon,        // :
    Question,     // ?
    Arrow,        // =>
    Spread,       // ...
    OptionalChain, // ?.

    // Template literals
    TemplateHead(String),    // `text${
    TemplateMiddle(String),  // }text${
    TemplateTail(String),    // }text`
    TemplateNoSub(String),   // `text` (no substitutions)

    // Special
    Eof,
}

impl TokenKind {
    /// Check if this token is a keyword.
    pub fn is_keyword(&self) -> bool {
        matches!(
            self,
            TokenKind::Let
                | TokenKind::Const
                | TokenKind::Var
                | TokenKind::Function
                | TokenKind::Return
                | TokenKind::If
                | TokenKind::Else
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Do
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Switch
                | TokenKind::Case
                | TokenKind::Default
                | TokenKind::New
                | TokenKind::Delete
                | TokenKind::Typeof
                | TokenKind::Void
                | TokenKind::In
                | TokenKind::Of
                | TokenKind::Instanceof
                | TokenKind::This
                | TokenKind::Throw
                | TokenKind::Try
                | TokenKind::Catch
                | TokenKind::Finally
                | TokenKind::Async
                | TokenKind::Await
                | TokenKind::Yield
                | TokenKind::Import
                | TokenKind::Export
                | TokenKind::From
                | TokenKind::Class
                | TokenKind::Extends
                | TokenKind::Super
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Null
                | TokenKind::Undefined
        )
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Number(n) => write!(f, "{}", n),
            TokenKind::String(s) => write!(f, "\"{}\"", s),
            TokenKind::Identifier(s) => write!(f, "{}", s),
            TokenKind::Let => write!(f, "let"),
            TokenKind::Const => write!(f, "const"),
            TokenKind::Var => write!(f, "var"),
            TokenKind::Function => write!(f, "function"),
            TokenKind::Return => write!(f, "return"),
            TokenKind::If => write!(f, "if"),
            TokenKind::Else => write!(f, "else"),
            TokenKind::While => write!(f, "while"),
            TokenKind::For => write!(f, "for"),
            TokenKind::Do => write!(f, "do"),
            TokenKind::Break => write!(f, "break"),
            TokenKind::Continue => write!(f, "continue"),
            TokenKind::Switch => write!(f, "switch"),
            TokenKind::Case => write!(f, "case"),
            TokenKind::Default => write!(f, "default"),
            TokenKind::New => write!(f, "new"),
            TokenKind::Delete => write!(f, "delete"),
            TokenKind::Typeof => write!(f, "typeof"),
            TokenKind::Void => write!(f, "void"),
            TokenKind::In => write!(f, "in"),
            TokenKind::Of => write!(f, "of"),
            TokenKind::Instanceof => write!(f, "instanceof"),
            TokenKind::This => write!(f, "this"),
            TokenKind::Throw => write!(f, "throw"),
            TokenKind::Try => write!(f, "try"),
            TokenKind::Catch => write!(f, "catch"),
            TokenKind::Finally => write!(f, "finally"),
            TokenKind::Async => write!(f, "async"),
            TokenKind::Await => write!(f, "await"),
            TokenKind::Yield => write!(f, "yield"),
            TokenKind::Import => write!(f, "import"),
            TokenKind::Export => write!(f, "export"),
            TokenKind::From => write!(f, "from"),
            TokenKind::Class => write!(f, "class"),
            TokenKind::Extends => write!(f, "extends"),
            TokenKind::Super => write!(f, "super"),
            TokenKind::True => write!(f, "true"),
            TokenKind::False => write!(f, "false"),
            TokenKind::Null => write!(f, "null"),
            TokenKind::Undefined => write!(f, "undefined"),
            TokenKind::Plus => write!(f, "+"),
            TokenKind::Minus => write!(f, "-"),
            TokenKind::Star => write!(f, "*"),
            TokenKind::Slash => write!(f, "/"),
            TokenKind::Percent => write!(f, "%"),
            TokenKind::StarStar => write!(f, "**"),
            TokenKind::Assign => write!(f, "="),
            TokenKind::PlusAssign => write!(f, "+="),
            TokenKind::MinusAssign => write!(f, "-="),
            TokenKind::StarAssign => write!(f, "*="),
            TokenKind::SlashAssign => write!(f, "/="),
            TokenKind::PercentAssign => write!(f, "%="),
            TokenKind::Equal => write!(f, "=="),
            TokenKind::NotEqual => write!(f, "!="),
            TokenKind::StrictEqual => write!(f, "==="),
            TokenKind::StrictNotEqual => write!(f, "!=="),
            TokenKind::LessThan => write!(f, "<"),
            TokenKind::GreaterThan => write!(f, ">"),
            TokenKind::LessEqual => write!(f, "<="),
            TokenKind::GreaterEqual => write!(f, ">="),
            TokenKind::And => write!(f, "&&"),
            TokenKind::Or => write!(f, "||"),
            TokenKind::Not => write!(f, "!"),
            TokenKind::NullishCoalesce => write!(f, "??"),
            TokenKind::BitAnd => write!(f, "&"),
            TokenKind::BitOr => write!(f, "|"),
            TokenKind::BitXor => write!(f, "^"),
            TokenKind::BitNot => write!(f, "~"),
            TokenKind::ShiftLeft => write!(f, "<<"),
            TokenKind::ShiftRight => write!(f, ">>"),
            TokenKind::UShiftRight => write!(f, ">>>"),
            TokenKind::PlusPlus => write!(f, "++"),
            TokenKind::MinusMinus => write!(f, "--"),
            TokenKind::LeftParen => write!(f, "("),
            TokenKind::RightParen => write!(f, ")"),
            TokenKind::LeftBrace => write!(f, "{{"),
            TokenKind::RightBrace => write!(f, "}}"),
            TokenKind::LeftBracket => write!(f, "["),
            TokenKind::RightBracket => write!(f, "]"),
            TokenKind::Semicolon => write!(f, ";"),
            TokenKind::Comma => write!(f, ","),
            TokenKind::Dot => write!(f, "."),
            TokenKind::Colon => write!(f, ":"),
            TokenKind::Question => write!(f, "?"),
            TokenKind::Arrow => write!(f, "=>"),
            TokenKind::Spread => write!(f, "..."),
            TokenKind::OptionalChain => write!(f, "?."),
            TokenKind::TemplateHead(s) => write!(f, "`{}${{", s),
            TokenKind::TemplateMiddle(s) => write!(f, "}}{}${{", s),
            TokenKind::TemplateTail(s) => write!(f, "}}{}`", s),
            TokenKind::TemplateNoSub(s) => write!(f, "`{}`", s),
            TokenKind::Eof => write!(f, "<EOF>"),
        }
    }
}

/// Look up a keyword from an identifier string.
pub fn lookup_keyword(ident: &str) -> Option<TokenKind> {
    match ident {
        "let" => Some(TokenKind::Let),
        "const" => Some(TokenKind::Const),
        "var" => Some(TokenKind::Var),
        "function" => Some(TokenKind::Function),
        "return" => Some(TokenKind::Return),
        "if" => Some(TokenKind::If),
        "else" => Some(TokenKind::Else),
        "while" => Some(TokenKind::While),
        "for" => Some(TokenKind::For),
        "do" => Some(TokenKind::Do),
        "break" => Some(TokenKind::Break),
        "continue" => Some(TokenKind::Continue),
        "switch" => Some(TokenKind::Switch),
        "case" => Some(TokenKind::Case),
        "default" => Some(TokenKind::Default),
        "new" => Some(TokenKind::New),
        "delete" => Some(TokenKind::Delete),
        "typeof" => Some(TokenKind::Typeof),
        "void" => Some(TokenKind::Void),
        "in" => Some(TokenKind::In),
        "of" => Some(TokenKind::Of),
        "instanceof" => Some(TokenKind::Instanceof),
        "this" => Some(TokenKind::This),
        "throw" => Some(TokenKind::Throw),
        "try" => Some(TokenKind::Try),
        "catch" => Some(TokenKind::Catch),
        "finally" => Some(TokenKind::Finally),
        "async" => Some(TokenKind::Async),
        "await" => Some(TokenKind::Await),
        "yield" => Some(TokenKind::Yield),
        "import" => Some(TokenKind::Import),
        "export" => Some(TokenKind::Export),
        "from" => Some(TokenKind::From),
        "class" => Some(TokenKind::Class),
        "extends" => Some(TokenKind::Extends),
        "super" => Some(TokenKind::Super),
        "true" => Some(TokenKind::True),
        "false" => Some(TokenKind::False),
        "null" => Some(TokenKind::Null),
        "undefined" => Some(TokenKind::Undefined),
        _ => None,
    }
}
