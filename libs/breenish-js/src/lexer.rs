//! Tokenizer for the breenish-js engine.
//!
//! Converts source text into a stream of tokens using a single-pass scanner.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::{JsError, JsResult};
use crate::token::{lookup_keyword, Span, Token, TokenKind};

/// The lexer converts source code into tokens.
pub struct Lexer<'a> {
    source: &'a [u8],
    pos: usize,
    line: u32,
    column: u32,
    /// Buffered tokens for lookahead.
    tokens: Vec<Token>,
    token_pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            pos: 0,
            line: 1,
            column: 1,
            tokens: Vec::new(),
            token_pos: 0,
        }
    }

    /// Tokenize the entire source into a token buffer.
    pub fn tokenize_all(&mut self) -> JsResult<()> {
        // Track template literal nesting depth so we know when a `}` closes
        // a template expression rather than a block.
        let mut template_depth: usize = 0;
        // Track brace depth within each template expression to handle
        // nested blocks like `${obj ? {a:1} : {b:2}}`
        let mut brace_depth_stack: Vec<usize> = Vec::new();

        loop {
            // If we're inside a template expression and hit a `}` that closes it,
            // scan the template continuation instead of emitting RightBrace.
            if template_depth > 0 {
                if let Some(&depth) = brace_depth_stack.last() {
                    if depth == 0 && self.peek_byte() == Some(b'}') {
                        // This `}` closes the template expression
                        self.advance(); // consume '}'
                        let tok = self.scan_template_continuation()?;
                        template_depth -= 1;
                        brace_depth_stack.pop();
                        match &tok.kind {
                            TokenKind::TemplateMiddle(_) => {
                                // Another expression follows
                                template_depth += 1;
                                brace_depth_stack.push(0);
                            }
                            TokenKind::TemplateTail(_) => {
                                // Template is done
                            }
                            _ => {}
                        }
                        let is_eof = tok.kind == TokenKind::Eof;
                        self.tokens.push(tok);
                        if is_eof {
                            break;
                        }
                        continue;
                    }
                }
            }

            let tok = self.scan_token()?;

            // Track template and brace depth
            match &tok.kind {
                TokenKind::TemplateHead(_) => {
                    template_depth += 1;
                    brace_depth_stack.push(0);
                }
                TokenKind::LeftBrace if template_depth > 0 => {
                    if let Some(depth) = brace_depth_stack.last_mut() {
                        *depth += 1;
                    }
                }
                TokenKind::RightBrace if template_depth > 0 => {
                    if let Some(depth) = brace_depth_stack.last_mut() {
                        *depth -= 1;
                    }
                }
                _ => {}
            }

            let is_eof = tok.kind == TokenKind::Eof;
            self.tokens.push(tok);
            if is_eof {
                break;
            }
        }
        self.token_pos = 0;
        Ok(())
    }

    /// Get the next token from the buffer.
    pub fn next_token(&mut self) -> &Token {
        if self.token_pos < self.tokens.len() {
            let tok = &self.tokens[self.token_pos];
            self.token_pos += 1;
            tok
        } else {
            // Return EOF
            &self.tokens[self.tokens.len() - 1]
        }
    }

    /// Peek at the current token without advancing.
    pub fn peek(&self) -> &Token {
        if self.token_pos < self.tokens.len() {
            &self.tokens[self.token_pos]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    /// Peek at the token N positions ahead.
    pub fn peek_ahead(&self, n: usize) -> &Token {
        let idx = self.token_pos + n;
        if idx < self.tokens.len() {
            &self.tokens[idx]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    /// Save the current token position (for backtracking).
    pub fn save_pos(&self) -> usize {
        self.token_pos
    }

    /// Restore the token position (for backtracking).
    pub fn restore_pos(&mut self, pos: usize) {
        self.token_pos = pos;
    }

    /// Check if the current token matches the given kind and advance if so.
    pub fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek().kind == *kind {
            self.next_token();
            true
        } else {
            false
        }
    }

    /// Expect the current token to be the given kind, advance and return it.
    pub fn expect(&mut self, kind: &TokenKind) -> JsResult<Token> {
        let tok = self.peek().clone();
        if tok.kind == *kind {
            self.next_token();
            Ok(tok)
        } else {
            Err(JsError::syntax(
                alloc::format!("expected '{}', got '{}'", kind, tok.kind),
                tok.span.line,
                tok.span.column,
            ))
        }
    }

    // --- Scanner ---

    fn peek_byte(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    fn peek_byte_at(&self, offset: usize) -> Option<u8> {
        self.source.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let b = self.source.get(self.pos).copied()?;
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(b)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek_byte() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => {
                    self.advance();
                }
                Some(b'/') => {
                    if self.peek_byte_at(1) == Some(b'/') {
                        // Line comment
                        self.advance();
                        self.advance();
                        while let Some(b) = self.peek_byte() {
                            if b == b'\n' {
                                break;
                            }
                            self.advance();
                        }
                    } else if self.peek_byte_at(1) == Some(b'*') {
                        // Block comment
                        self.advance();
                        self.advance();
                        loop {
                            match self.advance() {
                                Some(b'*') if self.peek_byte() == Some(b'/') => {
                                    self.advance();
                                    break;
                                }
                                None => break,
                                _ => {}
                            }
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
    }

    fn scan_token(&mut self) -> JsResult<Token> {
        self.skip_whitespace_and_comments();

        let start = self.pos as u32;
        let line = self.line;
        let column = self.column;

        let Some(b) = self.advance() else {
            return Ok(Token::new(TokenKind::Eof, Span::new(start, start, line, column)));
        };

        let kind = match b {
            // Identifiers and keywords
            b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => {
                let mut ident = String::new();
                ident.push(b as char);
                while let Some(c) = self.peek_byte() {
                    if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' {
                        ident.push(c as char);
                        self.advance();
                    } else {
                        break;
                    }
                }
                lookup_keyword(&ident).unwrap_or(TokenKind::Identifier(ident))
            }

            // Numbers
            b'0'..=b'9' => self.scan_number(b)?,

            // Strings
            b'"' | b'\'' => self.scan_string(b)?,

            // Template literals
            b'`' => self.scan_template_start()?,

            // Operators and punctuation
            b'(' => TokenKind::LeftParen,
            b')' => TokenKind::RightParen,
            b'{' => TokenKind::LeftBrace,
            b'}' => TokenKind::RightBrace,
            b'[' => TokenKind::LeftBracket,
            b']' => TokenKind::RightBracket,
            b';' => TokenKind::Semicolon,
            b',' => TokenKind::Comma,
            b':' => TokenKind::Colon,
            b'~' => TokenKind::BitNot,

            b'.' => {
                if self.peek_byte() == Some(b'.') && self.peek_byte_at(1) == Some(b'.') {
                    self.advance();
                    self.advance();
                    TokenKind::Spread
                } else if matches!(self.peek_byte(), Some(b'0'..=b'9')) {
                    // .5 style number
                    self.scan_number_after_dot(0.0)?
                } else {
                    TokenKind::Dot
                }
            }

            b'+' => {
                if self.peek_byte() == Some(b'+') {
                    self.advance();
                    TokenKind::PlusPlus
                } else if self.peek_byte() == Some(b'=') {
                    self.advance();
                    TokenKind::PlusAssign
                } else {
                    TokenKind::Plus
                }
            }

            b'-' => {
                if self.peek_byte() == Some(b'-') {
                    self.advance();
                    TokenKind::MinusMinus
                } else if self.peek_byte() == Some(b'=') {
                    self.advance();
                    TokenKind::MinusAssign
                } else {
                    TokenKind::Minus
                }
            }

            b'*' => {
                if self.peek_byte() == Some(b'*') {
                    self.advance();
                    TokenKind::StarStar
                } else if self.peek_byte() == Some(b'=') {
                    self.advance();
                    TokenKind::StarAssign
                } else {
                    TokenKind::Star
                }
            }

            b'/' => {
                if self.peek_byte() == Some(b'=') {
                    self.advance();
                    TokenKind::SlashAssign
                } else {
                    TokenKind::Slash
                }
            }

            b'%' => {
                if self.peek_byte() == Some(b'=') {
                    self.advance();
                    TokenKind::PercentAssign
                } else {
                    TokenKind::Percent
                }
            }

            b'=' => {
                if self.peek_byte() == Some(b'=') {
                    self.advance();
                    if self.peek_byte() == Some(b'=') {
                        self.advance();
                        TokenKind::StrictEqual
                    } else {
                        TokenKind::Equal
                    }
                } else if self.peek_byte() == Some(b'>') {
                    self.advance();
                    TokenKind::Arrow
                } else {
                    TokenKind::Assign
                }
            }

            b'!' => {
                if self.peek_byte() == Some(b'=') {
                    self.advance();
                    if self.peek_byte() == Some(b'=') {
                        self.advance();
                        TokenKind::StrictNotEqual
                    } else {
                        TokenKind::NotEqual
                    }
                } else {
                    TokenKind::Not
                }
            }

            b'<' => {
                if self.peek_byte() == Some(b'=') {
                    self.advance();
                    TokenKind::LessEqual
                } else if self.peek_byte() == Some(b'<') {
                    self.advance();
                    TokenKind::ShiftLeft
                } else {
                    TokenKind::LessThan
                }
            }

            b'>' => {
                if self.peek_byte() == Some(b'=') {
                    self.advance();
                    TokenKind::GreaterEqual
                } else if self.peek_byte() == Some(b'>') {
                    self.advance();
                    if self.peek_byte() == Some(b'>') {
                        self.advance();
                        TokenKind::UShiftRight
                    } else {
                        TokenKind::ShiftRight
                    }
                } else {
                    TokenKind::GreaterThan
                }
            }

            b'&' => {
                if self.peek_byte() == Some(b'&') {
                    self.advance();
                    TokenKind::And
                } else {
                    TokenKind::BitAnd
                }
            }

            b'|' => {
                if self.peek_byte() == Some(b'|') {
                    self.advance();
                    TokenKind::Or
                } else {
                    TokenKind::BitOr
                }
            }

            b'^' => TokenKind::BitXor,

            b'?' => {
                if self.peek_byte() == Some(b'?') {
                    self.advance();
                    TokenKind::NullishCoalesce
                } else if self.peek_byte() == Some(b'.') {
                    self.advance();
                    TokenKind::OptionalChain
                } else {
                    TokenKind::Question
                }
            }

            _ => {
                return Err(JsError::syntax(
                    alloc::format!("unexpected character '{}'", b as char),
                    line,
                    column,
                ));
            }
        };

        let end = self.pos as u32;
        Ok(Token::new(kind, Span::new(start, end, line, column)))
    }

    fn scan_number(&mut self, first: u8) -> JsResult<TokenKind> {
        let mut int_part: f64 = (first - b'0') as f64;

        // Check for hex, octal, binary
        if first == b'0' {
            match self.peek_byte() {
                Some(b'x' | b'X') => {
                    self.advance();
                    return self.scan_hex_number();
                }
                Some(b'b' | b'B') => {
                    self.advance();
                    return self.scan_binary_number();
                }
                Some(b'o' | b'O') => {
                    self.advance();
                    return self.scan_octal_number();
                }
                _ => {}
            }
        }

        // Integer part
        while let Some(c) = self.peek_byte() {
            if c.is_ascii_digit() {
                int_part = int_part * 10.0 + (c - b'0') as f64;
                self.advance();
            } else if c == b'_' {
                // Numeric separator
                self.advance();
            } else {
                break;
            }
        }

        // Fractional part
        if self.peek_byte() == Some(b'.') && matches!(self.peek_byte_at(1), Some(b'0'..=b'9')) {
            self.advance(); // consume '.'
            return self.scan_number_after_dot(int_part);
        }

        // Exponent
        if matches!(self.peek_byte(), Some(b'e' | b'E')) {
            return self.scan_exponent(int_part);
        }

        Ok(TokenKind::Number(int_part))
    }

    fn scan_number_after_dot(&mut self, int_part: f64) -> JsResult<TokenKind> {
        let mut frac = 0.0_f64;
        let mut frac_div = 1.0_f64;
        while let Some(c) = self.peek_byte() {
            if c.is_ascii_digit() {
                frac = frac * 10.0 + (c - b'0') as f64;
                frac_div *= 10.0;
                self.advance();
            } else if c == b'_' {
                self.advance();
            } else {
                break;
            }
        }
        let value = int_part + frac / frac_div;

        if matches!(self.peek_byte(), Some(b'e' | b'E')) {
            return self.scan_exponent(value);
        }

        Ok(TokenKind::Number(value))
    }

    fn scan_exponent(&mut self, base: f64) -> JsResult<TokenKind> {
        self.advance(); // consume 'e' or 'E'
        let negative = if self.peek_byte() == Some(b'-') {
            self.advance();
            true
        } else if self.peek_byte() == Some(b'+') {
            self.advance();
            false
        } else {
            false
        };
        let mut exp: i32 = 0;
        while let Some(c) = self.peek_byte() {
            if c.is_ascii_digit() {
                exp = exp * 10 + (c - b'0') as i32;
                self.advance();
            } else {
                break;
            }
        }
        if negative {
            exp = -exp;
        }
        let value = base * 10.0_f64.powi(exp);
        Ok(TokenKind::Number(value))
    }

    fn scan_hex_number(&mut self) -> JsResult<TokenKind> {
        let mut value: u64 = 0;
        while let Some(c) = self.peek_byte() {
            if c.is_ascii_hexdigit() {
                let digit = match c {
                    b'0'..=b'9' => (c - b'0') as u64,
                    b'a'..=b'f' => (c - b'a' + 10) as u64,
                    b'A'..=b'F' => (c - b'A' + 10) as u64,
                    _ => unreachable!(),
                };
                value = value * 16 + digit;
                self.advance();
            } else if c == b'_' {
                self.advance();
            } else {
                break;
            }
        }
        Ok(TokenKind::Number(value as f64))
    }

    fn scan_binary_number(&mut self) -> JsResult<TokenKind> {
        let mut value: u64 = 0;
        while let Some(c) = self.peek_byte() {
            if c == b'0' || c == b'1' {
                value = value * 2 + (c - b'0') as u64;
                self.advance();
            } else if c == b'_' {
                self.advance();
            } else {
                break;
            }
        }
        Ok(TokenKind::Number(value as f64))
    }

    fn scan_octal_number(&mut self) -> JsResult<TokenKind> {
        let mut value: u64 = 0;
        while let Some(c) = self.peek_byte() {
            if (b'0'..=b'7').contains(&c) {
                value = value * 8 + (c - b'0') as u64;
                self.advance();
            } else if c == b'_' {
                self.advance();
            } else {
                break;
            }
        }
        Ok(TokenKind::Number(value as f64))
    }

    fn scan_string(&mut self, quote: u8) -> JsResult<TokenKind> {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    return Err(JsError::syntax(
                        "unterminated string literal",
                        self.line,
                        self.column,
                    ));
                }
                Some(b) if b == quote => break,
                Some(b'\\') => {
                    let escaped = self.scan_escape_sequence()?;
                    s.push(escaped);
                }
                Some(b) => {
                    s.push(b as char);
                }
            }
        }
        Ok(TokenKind::String(s))
    }

    fn scan_escape_sequence(&mut self) -> JsResult<char> {
        match self.advance() {
            Some(b'n') => Ok('\n'),
            Some(b't') => Ok('\t'),
            Some(b'r') => Ok('\r'),
            Some(b'\\') => Ok('\\'),
            Some(b'\'') => Ok('\''),
            Some(b'"') => Ok('"'),
            Some(b'`') => Ok('`'),
            Some(b'0') => Ok('\0'),
            Some(b'$') => Ok('$'),
            Some(b'u') => self.scan_unicode_escape(),
            Some(b) => Ok(b as char),
            None => Err(JsError::syntax(
                "unterminated escape sequence",
                self.line,
                self.column,
            )),
        }
    }

    fn scan_unicode_escape(&mut self) -> JsResult<char> {
        if self.peek_byte() == Some(b'{') {
            self.advance();
            let mut code: u32 = 0;
            while let Some(c) = self.peek_byte() {
                if c == b'}' {
                    self.advance();
                    break;
                }
                if c.is_ascii_hexdigit() {
                    let digit = match c {
                        b'0'..=b'9' => (c - b'0') as u32,
                        b'a'..=b'f' => (c - b'a' + 10) as u32,
                        b'A'..=b'F' => (c - b'A' + 10) as u32,
                        _ => unreachable!(),
                    };
                    code = code * 16 + digit;
                    self.advance();
                } else {
                    break;
                }
            }
            char::from_u32(code).ok_or_else(|| {
                JsError::syntax("invalid unicode escape", self.line, self.column)
            })
        } else {
            let mut code: u32 = 0;
            for _ in 0..4 {
                match self.advance() {
                    Some(c) if c.is_ascii_hexdigit() => {
                        let digit = match c {
                            b'0'..=b'9' => (c - b'0') as u32,
                            b'a'..=b'f' => (c - b'a' + 10) as u32,
                            b'A'..=b'F' => (c - b'A' + 10) as u32,
                            _ => unreachable!(),
                        };
                        code = code * 16 + digit;
                    }
                    _ => {
                        return Err(JsError::syntax(
                            "invalid unicode escape",
                            self.line,
                            self.column,
                        ));
                    }
                }
            }
            char::from_u32(code).ok_or_else(|| {
                JsError::syntax("invalid unicode escape", self.line, self.column)
            })
        }
    }

    fn scan_template_start(&mut self) -> JsResult<TokenKind> {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    return Err(JsError::syntax(
                        "unterminated template literal",
                        self.line,
                        self.column,
                    ));
                }
                Some(b'`') => {
                    return Ok(TokenKind::TemplateNoSub(s));
                }
                Some(b'$') if self.peek_byte() == Some(b'{') => {
                    self.advance(); // consume '{'
                    return Ok(TokenKind::TemplateHead(s));
                }
                Some(b'\\') => {
                    let escaped = self.scan_escape_sequence()?;
                    s.push(escaped);
                }
                Some(b) => {
                    s.push(b as char);
                }
            }
        }
    }

    /// Scan template continuation after `}` in template literal.
    /// Called by the compiler when it encounters `}` during template parsing.
    pub fn scan_template_continuation(&mut self) -> JsResult<Token> {
        let start = self.pos as u32;
        let line = self.line;
        let column = self.column;
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    return Err(JsError::syntax(
                        "unterminated template literal",
                        self.line,
                        self.column,
                    ));
                }
                Some(b'`') => {
                    let end = self.pos as u32;
                    return Ok(Token::new(
                        TokenKind::TemplateTail(s),
                        Span::new(start, end, line, column),
                    ));
                }
                Some(b'$') if self.peek_byte() == Some(b'{') => {
                    self.advance();
                    let end = self.pos as u32;
                    return Ok(Token::new(
                        TokenKind::TemplateMiddle(s),
                        Span::new(start, end, line, column),
                    ));
                }
                Some(b'\\') => {
                    let escaped = self.scan_escape_sequence()?;
                    s.push(escaped);
                }
                Some(b) => {
                    s.push(b as char);
                }
            }
        }
    }
}
