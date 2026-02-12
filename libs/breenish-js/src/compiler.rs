//! Source-to-bytecode compiler for the breenish-js engine.
//!
//! This is a recursive-descent parser that emits bytecode directly,
//! without constructing an AST. This follows the QuickJS/JerryScript model:
//! shell scripts parse once and run once, so skipping the AST saves memory.

use alloc::string::String;
use alloc::vec::Vec;

use crate::bytecode::{CodeBlock, Constant, Op};
use crate::error::{JsError, JsResult};
use crate::lexer::Lexer;
use crate::string::StringPool;
use crate::token::TokenKind;

/// A local variable in the current scope.
#[derive(Debug, Clone)]
struct Local {
    name: String,
    slot: u16,
    is_const: bool,
}

/// A scope for tracking local variables.
#[derive(Debug)]
struct Scope {
    locals: Vec<Local>,
    base_slot: u16,
}

/// Loop context for break/continue.
#[derive(Debug, Clone)]
struct LoopContext {
    /// Offsets of break jumps that need patching.
    break_jumps: Vec<usize>,
    /// The bytecode offset of the loop start (for continue).
    continue_target: usize,
}

/// The compiler transforms source code into bytecode.
pub struct Compiler<'a> {
    lexer: Lexer<'a>,
    strings: StringPool,
    code: CodeBlock,
    scopes: Vec<Scope>,
    next_slot: u16,
    loop_stack: Vec<LoopContext>,
    /// Function table for nested function definitions.
    functions: Vec<CodeBlock>,
}

impl<'a> Compiler<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            lexer: Lexer::new(source),
            strings: StringPool::new(),
            code: CodeBlock::new("<main>"),
            scopes: vec![Scope {
                locals: Vec::new(),
                base_slot: 0,
            }],
            next_slot: 0,
            loop_stack: Vec::new(),
            functions: Vec::new(),
        }
    }

    /// Compile the source and return the code block and string pool.
    pub fn compile(mut self) -> JsResult<(CodeBlock, StringPool, Vec<CodeBlock>)> {
        self.lexer.tokenize_all()?;
        self.compile_program()?;
        self.code.emit_op(Op::Halt);
        self.code.local_count = self.next_slot;
        Ok((self.code, self.strings, self.functions))
    }

    // --- Program and Statements ---

    fn compile_program(&mut self) -> JsResult<()> {
        while self.lexer.peek().kind != TokenKind::Eof {
            self.compile_statement()?;
        }
        Ok(())
    }

    fn compile_statement(&mut self) -> JsResult<()> {
        match &self.lexer.peek().kind {
            TokenKind::Let | TokenKind::Const => self.compile_variable_declaration()?,
            TokenKind::Var => self.compile_var_declaration()?,
            TokenKind::If => self.compile_if_statement()?,
            TokenKind::While => self.compile_while_statement()?,
            TokenKind::For => self.compile_for_statement()?,
            TokenKind::Do => self.compile_do_while_statement()?,
            TokenKind::Switch => self.compile_switch_statement()?,
            TokenKind::Function => self.compile_function_declaration()?,
            TokenKind::Return => self.compile_return_statement()?,
            TokenKind::Break => self.compile_break_statement()?,
            TokenKind::Continue => self.compile_continue_statement()?,
            TokenKind::LeftBrace => self.compile_block()?,
            TokenKind::Semicolon => {
                self.lexer.next_token();
            }
            _ => self.compile_expression_statement()?,
        }
        Ok(())
    }

    fn compile_variable_declaration(&mut self) -> JsResult<()> {
        let is_const = self.lexer.peek().kind == TokenKind::Const;
        self.lexer.next_token(); // consume 'let' or 'const'

        loop {
            let tok = self.lexer.next_token().clone();
            let name = match &tok.kind {
                TokenKind::Identifier(name) => name.clone(),
                _ => {
                    return Err(JsError::syntax(
                        "expected variable name",
                        tok.span.line,
                        tok.span.column,
                    ));
                }
            };

            let slot = self.declare_local(name, is_const);

            if self.lexer.eat(&TokenKind::Assign) {
                self.compile_expression()?;
            } else if is_const {
                return Err(JsError::syntax(
                    "const declaration must be initialized",
                    tok.span.line,
                    tok.span.column,
                ));
            } else {
                self.code.emit_op(Op::LoadConst);
                let idx = self.code.add_constant(Constant::Number(f64::NAN));
                self.code.emit((idx >> 8) as u8);
                self.code.emit(idx as u8);
            }

            self.code.emit_op_u16(Op::StoreLocal, slot);

            if !self.lexer.eat(&TokenKind::Comma) {
                break;
            }
        }

        self.eat_semicolon();
        Ok(())
    }

    fn compile_var_declaration(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume 'var'

        loop {
            let tok = self.lexer.next_token().clone();
            let name = match &tok.kind {
                TokenKind::Identifier(name) => name.clone(),
                _ => {
                    return Err(JsError::syntax(
                        "expected variable name",
                        tok.span.line,
                        tok.span.column,
                    ));
                }
            };

            // var is function-scoped but we treat it like let in Phase 1
            let slot = self.declare_local(name, false);

            if self.lexer.eat(&TokenKind::Assign) {
                self.compile_expression()?;
            } else {
                // var defaults to undefined
                self.emit_undefined();
            }

            self.code.emit_op_u16(Op::StoreLocal, slot);

            if !self.lexer.eat(&TokenKind::Comma) {
                break;
            }
        }

        self.eat_semicolon();
        Ok(())
    }

    fn compile_if_statement(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume 'if'
        self.lexer.expect(&TokenKind::LeftParen)?;
        self.compile_expression()?;
        self.lexer.expect(&TokenKind::RightParen)?;

        let else_jump = self.code.emit_jump(Op::JumpIfFalse);
        self.compile_statement()?;

        if self.lexer.peek().kind == TokenKind::Else {
            self.lexer.next_token();
            let end_jump = self.code.emit_jump(Op::Jump);
            let else_target = self.code.current_offset();
            self.code.patch_jump(else_jump, else_target);
            self.compile_statement()?;
            let end_target = self.code.current_offset();
            self.code.patch_jump(end_jump, end_target);
        } else {
            let end_target = self.code.current_offset();
            self.code.patch_jump(else_jump, end_target);
        }

        Ok(())
    }

    fn compile_while_statement(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume 'while'

        let loop_start = self.code.current_offset();
        self.loop_stack.push(LoopContext {
            break_jumps: Vec::new(),
            continue_target: loop_start,
        });

        self.lexer.expect(&TokenKind::LeftParen)?;
        self.compile_expression()?;
        self.lexer.expect(&TokenKind::RightParen)?;

        let exit_jump = self.code.emit_jump(Op::JumpIfFalse);
        self.compile_statement()?;

        // Jump back to condition
        self.code.emit_op_u16(Op::Jump, loop_start as u16);

        let end = self.code.current_offset();
        self.code.patch_jump(exit_jump, end);

        // Patch break jumps
        let ctx = self.loop_stack.pop().unwrap();
        for brk in ctx.break_jumps {
            self.code.patch_jump(brk, end);
        }

        Ok(())
    }

    fn compile_for_statement(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume 'for'
        self.lexer.expect(&TokenKind::LeftParen)?;

        // Check for `for (let/const x of expr)` pattern
        if matches!(self.lexer.peek().kind, TokenKind::Let | TokenKind::Const) {
            // Check if this is for...of by peeking ahead: let IDENT of
            if let TokenKind::Identifier(_) = &self.lexer.peek_ahead(1).kind {
                if self.lexer.peek_ahead(2).kind == TokenKind::Of {
                    return self.compile_for_of_statement();
                }
            }
        }

        // Initializer
        match &self.lexer.peek().kind {
            TokenKind::Semicolon => {
                self.lexer.next_token();
            }
            TokenKind::Let | TokenKind::Const => {
                self.compile_variable_declaration()?;
            }
            TokenKind::Var => {
                self.compile_var_declaration()?;
            }
            _ => {
                self.compile_expression()?;
                self.code.emit_op(Op::Pop);
                self.eat_semicolon();
            }
        }

        let loop_start = self.code.current_offset();

        // Condition
        let exit_jump = if self.lexer.peek().kind != TokenKind::Semicolon {
            self.compile_expression()?;
            let j = self.code.emit_jump(Op::JumpIfFalse);
            self.eat_semicolon();
            Some(j)
        } else {
            self.lexer.next_token();
            None
        };

        // Increment - compile but skip over it initially
        let body_jump = self.code.emit_jump(Op::Jump);

        let increment_offset = self.code.current_offset();
        if self.lexer.peek().kind != TokenKind::RightParen {
            self.compile_expression()?;
            self.code.emit_op(Op::Pop);
        }
        self.lexer.expect(&TokenKind::RightParen)?;

        // Jump back to condition after increment
        self.code.emit_op_u16(Op::Jump, loop_start as u16);

        let body_start = self.code.current_offset();
        self.code.patch_jump(body_jump, body_start);

        self.loop_stack.push(LoopContext {
            break_jumps: Vec::new(),
            continue_target: increment_offset,
        });

        self.compile_statement()?;

        // Jump to increment
        self.code
            .emit_op_u16(Op::Jump, increment_offset as u16);

        let end = self.code.current_offset();

        if let Some(j) = exit_jump {
            self.code.patch_jump(j, end);
        }

        let ctx = self.loop_stack.pop().unwrap();
        for brk in ctx.break_jumps {
            self.code.patch_jump(brk, end);
        }

        Ok(())
    }

    /// Compile `for (let x of iterable) { body }`
    /// Currently supports arrays by iterating over indices.
    fn compile_for_of_statement(&mut self) -> JsResult<()> {
        let is_const = self.lexer.peek().kind == TokenKind::Const;
        self.lexer.next_token(); // consume 'let' or 'const'

        let tok = self.lexer.next_token().clone();
        let var_name = match &tok.kind {
            TokenKind::Identifier(n) => n.clone(),
            _ => {
                return Err(JsError::syntax(
                    "expected variable name in for...of",
                    tok.span.line,
                    tok.span.column,
                ));
            }
        };

        self.lexer.expect(&TokenKind::Of)?;

        // Compile the iterable expression
        self.compile_expression()?;
        self.lexer.expect(&TokenKind::RightParen)?;

        // Store iterable in a temp local
        let iterable_slot = self.declare_local(String::from("__for_of_iterable__"), false);
        self.code.emit_op_u16(Op::StoreLocal, iterable_slot);

        // Initialize index counter to 0
        let index_slot = self.declare_local(String::from("__for_of_index__"), false);
        let zero_idx = self.code.add_number(0.0);
        self.code.emit_op_u16(Op::LoadConst, zero_idx);
        self.code.emit_op_u16(Op::StoreLocal, index_slot);

        // Declare the loop variable
        let var_slot = self.declare_local(var_name, is_const);

        // Loop start: check index < iterable.length
        let loop_start = self.code.current_offset();

        // Load index
        self.code.emit_op_u16(Op::LoadLocal, index_slot);
        // Load iterable.length
        self.code.emit_op_u16(Op::LoadLocal, iterable_slot);
        let length_id = self.strings.intern("length");
        let length_idx = self.code.add_string(length_id.0);
        self.code.emit_op_u16(Op::GetPropertyConst, length_idx);
        // Compare: index < length
        self.code.emit_op(Op::LessThan);
        let exit_jump = self.code.emit_jump(Op::JumpIfFalse);

        // Set loop variable = iterable[index]
        self.code.emit_op_u16(Op::LoadLocal, iterable_slot);
        self.code.emit_op_u16(Op::LoadLocal, index_slot);
        self.code.emit_op(Op::GetIndex);
        self.code.emit_op_u16(Op::StoreLocal, var_slot);

        // Jump over the increment code (to the body)
        let body_jump = self.code.emit_jump(Op::Jump);

        // Increment code (continue target and end-of-body target)
        let increment_offset = self.code.current_offset();
        self.code.emit_op_u16(Op::LoadLocal, index_slot);
        let one_idx = self.code.add_number(1.0);
        self.code.emit_op_u16(Op::LoadConst, one_idx);
        self.code.emit_op(Op::Add);
        self.code.emit_op_u16(Op::StoreLocal, index_slot);
        // Jump back to condition check
        self.code.emit_op_u16(Op::Jump, loop_start as u16);

        // Body
        let body_start = self.code.current_offset();
        self.code.patch_jump(body_jump, body_start);

        self.loop_stack.push(LoopContext {
            break_jumps: Vec::new(),
            continue_target: increment_offset,
        });

        self.compile_statement()?;

        // Jump to increment
        self.code.emit_op_u16(Op::Jump, increment_offset as u16);

        let end = self.code.current_offset();
        self.code.patch_jump(exit_jump, end);

        // Patch break jumps
        let ctx = self.loop_stack.pop().unwrap();
        for brk in ctx.break_jumps {
            self.code.patch_jump(brk, end);
        }

        Ok(())
    }

    fn compile_do_while_statement(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume 'do'

        let loop_start = self.code.current_offset();
        self.loop_stack.push(LoopContext {
            break_jumps: Vec::new(),
            continue_target: loop_start, // Will be updated
        });

        self.compile_statement()?;

        // continue target is the condition check
        let condition_start = self.code.current_offset();
        if let Some(ctx) = self.loop_stack.last_mut() {
            ctx.continue_target = condition_start;
        }

        self.lexer.expect(&TokenKind::While)?;
        self.lexer.expect(&TokenKind::LeftParen)?;
        self.compile_expression()?;
        self.lexer.expect(&TokenKind::RightParen)?;

        // Jump back if true
        self.code.emit_op_u16(Op::JumpIfTrue, loop_start as u16);

        let end = self.code.current_offset();
        let ctx = self.loop_stack.pop().unwrap();
        for brk in ctx.break_jumps {
            self.code.patch_jump(brk, end);
        }

        self.eat_semicolon();
        Ok(())
    }

    fn compile_switch_statement(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume 'switch'
        self.lexer.expect(&TokenKind::LeftParen)?;
        self.compile_expression()?; // discriminant on stack
        self.lexer.expect(&TokenKind::RightParen)?;
        self.lexer.expect(&TokenKind::LeftBrace)?;

        // Compile switch with proper fallthrough support.
        // Strategy: emit all case comparisons first (each jumps to its body),
        // then emit all bodies in order. Fallthrough works naturally because
        // bodies are sequential.
        //
        // Layout:
        //   [discriminant on stack]
        //   Dup, case1_val, StrictEqual, JumpIfTrue -> body1
        //   Dup, case2_val, StrictEqual, JumpIfTrue -> body2
        //   ...
        //   Jump -> default_body (or end if no default)
        //   Pop discriminant
        //   body1: statements...
        //   body2: statements...
        //   default_body: statements...
        //   end:

        self.loop_stack.push(LoopContext {
            break_jumps: Vec::new(),
            continue_target: 0, // switch doesn't support continue
        });

        // We need to save and restore the lexer position to do two passes.
        // Since we use a pre-tokenized buffer, we can save/restore token_pos.
        let save_pos = self.lexer.save_pos();

        // First pass: emit comparisons and collect body start positions
        let mut case_body_jumps: Vec<usize> = Vec::new(); // JumpIfTrue targets to patch
        let mut default_index: Option<usize> = None;
        let mut case_count = 0usize;

        while self.lexer.peek().kind != TokenKind::RightBrace
            && self.lexer.peek().kind != TokenKind::Eof
        {
            match &self.lexer.peek().kind {
                TokenKind::Case => {
                    self.lexer.next_token(); // consume 'case'
                    // Dup discriminant, compile case value, compare
                    self.code.emit_op(Op::Dup);
                    self.compile_expression()?;
                    self.code.emit_op(Op::StrictEqual);
                    // JumpIfTrue to body (will be patched in second pass)
                    let jump = self.code.emit_jump(Op::JumpIfTrue);
                    case_body_jumps.push(jump);
                    self.lexer.expect(&TokenKind::Colon)?;
                    case_count += 1;

                    // Skip body tokens
                    while self.lexer.peek().kind != TokenKind::Case
                        && self.lexer.peek().kind != TokenKind::Default
                        && self.lexer.peek().kind != TokenKind::RightBrace
                        && self.lexer.peek().kind != TokenKind::Eof
                    {
                        self.lexer.next_token();
                    }
                }
                TokenKind::Default => {
                    self.lexer.next_token(); // consume 'default'
                    self.lexer.expect(&TokenKind::Colon)?;
                    default_index = Some(case_count);
                    case_count += 1;

                    // Skip body tokens
                    while self.lexer.peek().kind != TokenKind::Case
                        && self.lexer.peek().kind != TokenKind::RightBrace
                        && self.lexer.peek().kind != TokenKind::Eof
                    {
                        self.lexer.next_token();
                    }
                }
                _ => {
                    let tok = self.lexer.peek().clone();
                    return Err(JsError::syntax(
                        "expected 'case' or 'default'",
                        tok.span.line,
                        tok.span.column,
                    ));
                }
            }
        }

        // After all case comparisons, jump to default or end
        let default_or_end_jump = self.code.emit_jump(Op::Jump);

        // Pop discriminant before entering bodies
        self.code.emit_op(Op::Pop);

        // Second pass: emit bodies
        self.lexer.restore_pos(save_pos);
        let mut body_offsets: Vec<usize> = Vec::new();

        while self.lexer.peek().kind != TokenKind::RightBrace
            && self.lexer.peek().kind != TokenKind::Eof
        {
            match &self.lexer.peek().kind {
                TokenKind::Case => {
                    self.lexer.next_token(); // consume 'case'
                    // Skip case expression (already compiled in first pass)
                    self.skip_expression()?;
                    self.lexer.expect(&TokenKind::Colon)?;

                    body_offsets.push(self.code.current_offset());

                    // Compile body statements
                    while self.lexer.peek().kind != TokenKind::Case
                        && self.lexer.peek().kind != TokenKind::Default
                        && self.lexer.peek().kind != TokenKind::RightBrace
                        && self.lexer.peek().kind != TokenKind::Eof
                    {
                        self.compile_statement()?;
                    }
                }
                TokenKind::Default => {
                    self.lexer.next_token(); // consume 'default'
                    self.lexer.expect(&TokenKind::Colon)?;

                    body_offsets.push(self.code.current_offset());

                    // Compile default body
                    while self.lexer.peek().kind != TokenKind::Case
                        && self.lexer.peek().kind != TokenKind::RightBrace
                        && self.lexer.peek().kind != TokenKind::Eof
                    {
                        self.compile_statement()?;
                    }
                }
                _ => break,
            }
        }

        self.lexer.expect(&TokenKind::RightBrace)?;

        let end = self.code.current_offset();

        // Patch case jumps to their body offsets
        let mut case_jump_idx = 0;
        for (i, offset) in body_offsets.iter().enumerate() {
            if Some(i) == default_index {
                // Default doesn't have a case jump
                continue;
            }
            if case_jump_idx < case_body_jumps.len() {
                self.code.patch_jump(case_body_jumps[case_jump_idx], *offset);
                case_jump_idx += 1;
            }
        }

        // Patch default-or-end jump
        if let Some(di) = default_index {
            if di < body_offsets.len() {
                self.code.patch_jump(default_or_end_jump, body_offsets[di]);
            } else {
                self.code.patch_jump(default_or_end_jump, end);
            }
        } else {
            // No default: jump past the Pop and to end
            self.code.patch_jump(default_or_end_jump, end);
        }

        // Patch break jumps
        let ctx = self.loop_stack.pop().unwrap();
        for brk in ctx.break_jumps {
            self.code.patch_jump(brk, end);
        }

        Ok(())
    }

    /// Skip over an expression in the token stream without compiling it.
    /// Used during switch statement's first pass.
    fn skip_expression(&mut self) -> JsResult<()> {
        let mut depth = 0;
        loop {
            let kind = &self.lexer.peek().kind;
            match kind {
                TokenKind::Colon if depth == 0 => break,
                TokenKind::Semicolon if depth == 0 => break,
                TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace => {
                    depth += 1;
                    self.lexer.next_token();
                }
                TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    self.lexer.next_token();
                }
                TokenKind::Eof => break,
                _ => {
                    self.lexer.next_token();
                }
            }
        }
        Ok(())
    }

    fn compile_function_declaration(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume 'function'

        let tok = self.lexer.next_token().clone();
        let name = match &tok.kind {
            TokenKind::Identifier(name) => name.clone(),
            _ => {
                return Err(JsError::syntax(
                    "expected function name",
                    tok.span.line,
                    tok.span.column,
                ));
            }
        };

        let func_index = self.compile_function_body(&name)?;

        // Store function as both a local and a global so recursive calls
        // from within the function body can find it via global lookup.
        let slot = self.declare_local(name.clone(), false);
        let const_idx = self.code.add_constant(Constant::Function(func_index as u32));
        self.code.emit_op_u16(Op::LoadConst, const_idx);
        self.code.emit_op_u16(Op::StoreLocal, slot);

        // Also store as global for recursive access from nested scopes
        self.code.emit_op_u16(Op::LoadLocal, slot);
        let name_id = self.strings.intern(&name);
        let name_idx = self.code.add_string(name_id.0);
        self.code.emit_op_u16(Op::StoreGlobal, name_idx);

        Ok(())
    }

    fn compile_function_body(&mut self, name: &str) -> JsResult<usize> {
        self.lexer.expect(&TokenKind::LeftParen)?;

        // Parse parameter list
        let mut params: Vec<String> = Vec::new();
        if self.lexer.peek().kind != TokenKind::RightParen {
            loop {
                let tok = self.lexer.next_token().clone();
                match &tok.kind {
                    TokenKind::Identifier(n) => params.push(n.clone()),
                    _ => {
                        return Err(JsError::syntax(
                            "expected parameter name",
                            tok.span.line,
                            tok.span.column,
                        ));
                    }
                }
                if !self.lexer.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.lexer.expect(&TokenKind::RightParen)?;

        // Save current compiler state
        let prev_code = core::mem::replace(&mut self.code, CodeBlock::new(name));
        let prev_scopes = core::mem::replace(
            &mut self.scopes,
            vec![Scope {
                locals: Vec::new(),
                base_slot: 0,
            }],
        );
        let prev_next_slot = self.next_slot;
        self.next_slot = 0;

        // Declare parameters as locals
        for param in &params {
            self.declare_local(param.clone(), false);
        }

        // Compile function body
        self.lexer.expect(&TokenKind::LeftBrace)?;
        while self.lexer.peek().kind != TokenKind::RightBrace
            && self.lexer.peek().kind != TokenKind::Eof
        {
            self.compile_statement()?;
        }
        self.lexer.expect(&TokenKind::RightBrace)?;

        // Ensure function returns undefined if no explicit return
        self.emit_undefined();
        self.code.emit_op(Op::Return);

        self.code.local_count = self.next_slot;

        // Restore state
        let func_code = core::mem::replace(&mut self.code, prev_code);
        self.scopes = prev_scopes;
        self.next_slot = prev_next_slot;

        let func_index = self.functions.len();
        self.functions.push(func_code);
        Ok(func_index)
    }

    fn compile_return_statement(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume 'return'

        if self.lexer.peek().kind == TokenKind::Semicolon
            || self.lexer.peek().kind == TokenKind::RightBrace
            || self.lexer.peek().kind == TokenKind::Eof
        {
            self.emit_undefined();
        } else {
            self.compile_expression()?;
        }

        self.code.emit_op(Op::Return);
        self.eat_semicolon();
        Ok(())
    }

    fn compile_break_statement(&mut self) -> JsResult<()> {
        let tok = self.lexer.next_token().clone();
        if self.loop_stack.is_empty() {
            return Err(JsError::syntax(
                "break outside of loop",
                tok.span.line,
                tok.span.column,
            ));
        }
        let jump = self.code.emit_jump(Op::Jump);
        self.loop_stack.last_mut().unwrap().break_jumps.push(jump);
        self.eat_semicolon();
        Ok(())
    }

    fn compile_continue_statement(&mut self) -> JsResult<()> {
        let tok = self.lexer.next_token().clone();
        if self.loop_stack.is_empty() {
            return Err(JsError::syntax(
                "continue outside of loop",
                tok.span.line,
                tok.span.column,
            ));
        }
        let target = self.loop_stack.last().unwrap().continue_target;
        self.code.emit_op_u16(Op::Jump, target as u16);
        self.eat_semicolon();
        Ok(())
    }

    fn compile_block(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume '{'
        self.push_scope();

        while self.lexer.peek().kind != TokenKind::RightBrace
            && self.lexer.peek().kind != TokenKind::Eof
        {
            self.compile_statement()?;
        }

        self.lexer.expect(&TokenKind::RightBrace)?;
        self.pop_scope();
        Ok(())
    }

    fn compile_expression_statement(&mut self) -> JsResult<()> {
        self.compile_expression()?;
        self.code.emit_op(Op::Pop);
        self.eat_semicolon();
        Ok(())
    }

    // --- Expressions ---

    fn compile_expression(&mut self) -> JsResult<()> {
        self.compile_assignment()
    }

    fn compile_assignment(&mut self) -> JsResult<()> {
        // Check if this is an assignment target
        let tok = self.lexer.peek().clone();

        if let TokenKind::Identifier(name) = &tok.kind {
            let name = name.clone();
            // Look ahead for assignment operator
            let next = self.lexer.peek_ahead(1);
            match &next.kind {
                TokenKind::Assign => {
                    self.lexer.next_token(); // consume identifier
                    self.lexer.next_token(); // consume '='
                    self.compile_assignment()?;

                    if let Some((slot, _)) = self.resolve_local(&name) {
                        self.code.emit_op_u16(Op::StoreLocal, slot);
                        self.code.emit_op_u16(Op::LoadLocal, slot);
                    } else {
                        let name_id = self.strings.intern(&name);
                        let idx = self.code.add_string(name_id.0);
                        self.code.emit_op_u16(Op::StoreGlobal, idx);
                        self.code.emit_op_u16(Op::LoadGlobal, idx);
                    }
                    return Ok(());
                }
                TokenKind::PlusAssign
                | TokenKind::MinusAssign
                | TokenKind::StarAssign
                | TokenKind::SlashAssign
                | TokenKind::PercentAssign => {
                    let op = match &next.kind {
                        TokenKind::PlusAssign => Op::Add,
                        TokenKind::MinusAssign => Op::Sub,
                        TokenKind::StarAssign => Op::Mul,
                        TokenKind::SlashAssign => Op::Div,
                        TokenKind::PercentAssign => Op::Mod,
                        _ => unreachable!(),
                    };
                    self.lexer.next_token(); // consume identifier
                    self.lexer.next_token(); // consume compound assignment

                    // Load current value
                    if let Some((slot, _)) = self.resolve_local(&name) {
                        self.code.emit_op_u16(Op::LoadLocal, slot);
                    } else {
                        let name_id = self.strings.intern(&name);
                        let idx = self.code.add_string(name_id.0);
                        self.code.emit_op_u16(Op::LoadGlobal, idx);
                    }

                    // Compile RHS
                    self.compile_assignment()?;

                    // Apply operator
                    self.code.emit_op(op);

                    // Store back
                    if let Some((slot, _)) = self.resolve_local(&name) {
                        self.code.emit_op_u16(Op::StoreLocal, slot);
                        self.code.emit_op_u16(Op::LoadLocal, slot);
                    } else {
                        let name_id = self.strings.intern(&name);
                        let idx = self.code.add_string(name_id.0);
                        self.code.emit_op_u16(Op::StoreGlobal, idx);
                        self.code.emit_op_u16(Op::LoadGlobal, idx);
                    }
                    return Ok(());
                }
                _ => {}
            }
        }

        self.compile_ternary()
    }

    fn compile_ternary(&mut self) -> JsResult<()> {
        self.compile_or()?;

        if self.lexer.peek().kind == TokenKind::Question {
            self.lexer.next_token(); // consume '?'
            let else_jump = self.code.emit_jump(Op::JumpIfFalse);
            self.compile_assignment()?;
            let end_jump = self.code.emit_jump(Op::Jump);
            self.lexer.expect(&TokenKind::Colon)?;
            let else_target = self.code.current_offset();
            self.code.patch_jump(else_jump, else_target);
            self.compile_assignment()?;
            let end_target = self.code.current_offset();
            self.code.patch_jump(end_jump, end_target);
        }

        Ok(())
    }

    fn compile_or(&mut self) -> JsResult<()> {
        self.compile_and()?;

        while self.lexer.peek().kind == TokenKind::Or {
            self.lexer.next_token();
            // Short-circuit: if truthy, skip RHS
            self.code.emit_op(Op::Dup);
            let skip = self.code.emit_jump(Op::JumpIfTrue);
            self.code.emit_op(Op::Pop);
            self.compile_and()?;
            let end = self.code.current_offset();
            self.code.patch_jump(skip, end);
        }

        Ok(())
    }

    fn compile_and(&mut self) -> JsResult<()> {
        self.compile_equality()?;

        while self.lexer.peek().kind == TokenKind::And {
            self.lexer.next_token();
            // Short-circuit: if falsy, skip RHS
            self.code.emit_op(Op::Dup);
            let skip = self.code.emit_jump(Op::JumpIfFalse);
            self.code.emit_op(Op::Pop);
            self.compile_equality()?;
            let end = self.code.current_offset();
            self.code.patch_jump(skip, end);
        }

        Ok(())
    }

    fn compile_equality(&mut self) -> JsResult<()> {
        self.compile_comparison()?;

        loop {
            match &self.lexer.peek().kind {
                TokenKind::StrictEqual => {
                    self.lexer.next_token();
                    self.compile_comparison()?;
                    self.code.emit_op(Op::StrictEqual);
                }
                TokenKind::StrictNotEqual => {
                    self.lexer.next_token();
                    self.compile_comparison()?;
                    self.code.emit_op(Op::StrictNotEqual);
                }
                TokenKind::Equal => {
                    self.lexer.next_token();
                    self.compile_comparison()?;
                    self.code.emit_op(Op::Equal);
                }
                TokenKind::NotEqual => {
                    self.lexer.next_token();
                    self.compile_comparison()?;
                    self.code.emit_op(Op::NotEqual);
                }
                _ => break,
            }
        }

        Ok(())
    }

    fn compile_comparison(&mut self) -> JsResult<()> {
        self.compile_addition()?;

        loop {
            match &self.lexer.peek().kind {
                TokenKind::LessThan => {
                    self.lexer.next_token();
                    self.compile_addition()?;
                    self.code.emit_op(Op::LessThan);
                }
                TokenKind::GreaterThan => {
                    self.lexer.next_token();
                    self.compile_addition()?;
                    self.code.emit_op(Op::GreaterThan);
                }
                TokenKind::LessEqual => {
                    self.lexer.next_token();
                    self.compile_addition()?;
                    self.code.emit_op(Op::LessEqual);
                }
                TokenKind::GreaterEqual => {
                    self.lexer.next_token();
                    self.compile_addition()?;
                    self.code.emit_op(Op::GreaterEqual);
                }
                _ => break,
            }
        }

        Ok(())
    }

    fn compile_addition(&mut self) -> JsResult<()> {
        self.compile_multiplication()?;

        loop {
            match &self.lexer.peek().kind {
                TokenKind::Plus => {
                    self.lexer.next_token();
                    self.compile_multiplication()?;
                    self.code.emit_op(Op::Add);
                }
                TokenKind::Minus => {
                    self.lexer.next_token();
                    self.compile_multiplication()?;
                    self.code.emit_op(Op::Sub);
                }
                _ => break,
            }
        }

        Ok(())
    }

    fn compile_multiplication(&mut self) -> JsResult<()> {
        self.compile_unary()?;

        loop {
            match &self.lexer.peek().kind {
                TokenKind::Star => {
                    self.lexer.next_token();
                    self.compile_unary()?;
                    self.code.emit_op(Op::Mul);
                }
                TokenKind::Slash => {
                    self.lexer.next_token();
                    self.compile_unary()?;
                    self.code.emit_op(Op::Div);
                }
                TokenKind::Percent => {
                    self.lexer.next_token();
                    self.compile_unary()?;
                    self.code.emit_op(Op::Mod);
                }
                _ => break,
            }
        }

        Ok(())
    }

    fn compile_unary(&mut self) -> JsResult<()> {
        match &self.lexer.peek().kind {
            TokenKind::Minus => {
                self.lexer.next_token();
                self.compile_unary()?;
                self.code.emit_op(Op::Negate);
                Ok(())
            }
            TokenKind::Not => {
                self.lexer.next_token();
                self.compile_unary()?;
                self.code.emit_op(Op::Not);
                Ok(())
            }
            TokenKind::Typeof => {
                self.lexer.next_token();
                self.compile_unary()?;
                self.code.emit_op(Op::TypeOf);
                Ok(())
            }
            TokenKind::PlusPlus => {
                self.lexer.next_token();
                self.compile_prefix_update(Op::Add)?;
                Ok(())
            }
            TokenKind::MinusMinus => {
                self.lexer.next_token();
                self.compile_prefix_update(Op::Sub)?;
                Ok(())
            }
            _ => self.compile_postfix(),
        }
    }

    fn compile_prefix_update(&mut self, op: Op) -> JsResult<()> {
        let tok = self.lexer.next_token().clone();
        let name = match &tok.kind {
            TokenKind::Identifier(n) => n.clone(),
            _ => {
                return Err(JsError::syntax(
                    "expected variable name",
                    tok.span.line,
                    tok.span.column,
                ));
            }
        };

        // Load, add/sub 1, store, leave new value on stack
        if let Some((slot, _)) = self.resolve_local(&name) {
            self.code.emit_op_u16(Op::LoadLocal, slot);
            let one = self.code.add_number(1.0);
            self.code.emit_op_u16(Op::LoadConst, one);
            self.code.emit_op(op);
            self.code.emit_op_u16(Op::StoreLocal, slot);
            self.code.emit_op_u16(Op::LoadLocal, slot);
        } else {
            let name_id = self.strings.intern(&name);
            let idx = self.code.add_string(name_id.0);
            self.code.emit_op_u16(Op::LoadGlobal, idx);
            let one = self.code.add_number(1.0);
            self.code.emit_op_u16(Op::LoadConst, one);
            self.code.emit_op(op);
            self.code.emit_op_u16(Op::StoreGlobal, idx);
            self.code.emit_op_u16(Op::LoadGlobal, idx);
        }

        Ok(())
    }

    fn compile_postfix(&mut self) -> JsResult<()> {
        self.compile_call()?;

        // Postfix ++ and -- (Phase 1: handle simple identifier case)
        if matches!(
            self.lexer.peek().kind,
            TokenKind::PlusPlus | TokenKind::MinusMinus
        ) {
            // For Phase 1, just consume and ignore postfix ops on non-identifiers
            self.lexer.next_token();
        }

        Ok(())
    }

    fn compile_call(&mut self) -> JsResult<()> {
        self.compile_primary()?;

        // Handle postfix operations: calls, property access, indexing
        loop {
            match &self.lexer.peek().kind {
                TokenKind::LeftParen => {
                    self.lexer.next_token(); // consume '('
                    let mut argc: u8 = 0;
                    if self.lexer.peek().kind != TokenKind::RightParen {
                        loop {
                            self.compile_expression()?;
                            argc += 1;
                            if !self.lexer.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    self.lexer.expect(&TokenKind::RightParen)?;
                    self.code.emit_op_u8(Op::Call, argc);
                }
                TokenKind::Dot => {
                    self.lexer.next_token(); // consume '.'
                    let tok = self.lexer.next_token().clone();
                    let name = match &tok.kind {
                        TokenKind::Identifier(n) => n.clone(),
                        // Allow keywords as property names
                        _ if tok.kind.is_keyword() => alloc::format!("{}", tok.kind),
                        _ => {
                            return Err(JsError::syntax(
                                "expected property name",
                                tok.span.line,
                                tok.span.column,
                            ));
                        }
                    };

                    // Check for assignment to property: obj.prop = value
                    if self.lexer.peek().kind == TokenKind::Assign {
                        self.lexer.next_token(); // consume '='
                        self.compile_expression()?;
                        let name_id = self.strings.intern(&name);
                        let idx = self.code.add_string(name_id.0);
                        self.code.emit_op_u16(Op::SetPropertyConst, idx);
                    } else if self.lexer.peek().kind == TokenKind::LeftParen {
                        // Method call: obj.method(args)
                        self.lexer.next_token(); // consume '('
                        let mut argc: u8 = 0;
                        if self.lexer.peek().kind != TokenKind::RightParen {
                            loop {
                                self.compile_expression()?;
                                argc += 1;
                                if !self.lexer.eat(&TokenKind::Comma) {
                                    break;
                                }
                            }
                        }
                        self.lexer.expect(&TokenKind::RightParen)?;
                        let name_id = self.strings.intern(&name);
                        let idx = self.code.add_string(name_id.0);
                        self.code.emit_op_u16_u8(Op::CallMethod, idx, argc);
                    } else {
                        let name_id = self.strings.intern(&name);
                        let idx = self.code.add_string(name_id.0);
                        self.code.emit_op_u16(Op::GetPropertyConst, idx);
                    }
                }
                TokenKind::LeftBracket => {
                    self.lexer.next_token(); // consume '['
                    self.compile_expression()?;

                    if self.lexer.peek().kind == TokenKind::RightBracket
                        && self.lexer.peek_ahead(1).kind == TokenKind::Assign
                    {
                        // obj[key] = value
                        self.lexer.next_token(); // consume ']'
                        self.lexer.next_token(); // consume '='
                        self.compile_expression()?;
                        self.code.emit_op(Op::SetProperty);
                    } else {
                        self.lexer.expect(&TokenKind::RightBracket)?;
                        self.code.emit_op(Op::GetProperty);
                    }
                }
                _ => break,
            }
        }

        Ok(())
    }

    fn compile_primary(&mut self) -> JsResult<()> {
        let tok = self.lexer.peek().clone();
        match &tok.kind {
            TokenKind::Number(n) => {
                let n = *n;
                self.lexer.next_token();
                // Small integer optimization
                if n == n.floor() && n >= i32::MIN as f64 && n <= i32::MAX as f64 && !n.is_nan() {
                    let idx = self.code.add_number(n);
                    self.code.emit_op_u16(Op::LoadConst, idx);
                } else {
                    let idx = self.code.add_number(n);
                    self.code.emit_op_u16(Op::LoadConst, idx);
                }
                Ok(())
            }

            TokenKind::String(s) => {
                let s = s.clone();
                self.lexer.next_token();
                let id = self.strings.intern(&s);
                let idx = self.code.add_string(id.0);
                self.code.emit_op_u16(Op::LoadConst, idx);
                Ok(())
            }

            TokenKind::TemplateNoSub(s) => {
                let s = s.clone();
                self.lexer.next_token();
                let id = self.strings.intern(&s);
                let idx = self.code.add_string(id.0);
                self.code.emit_op_u16(Op::LoadConst, idx);
                Ok(())
            }

            TokenKind::TemplateHead(s) => {
                let s = s.clone();
                self.lexer.next_token();
                // Push the head string
                let id = self.strings.intern(&s);
                let idx = self.code.add_string(id.0);
                self.code.emit_op_u16(Op::LoadConst, idx);
                // Compile expression, concatenate
                self.compile_expression()?;
                self.code.emit_op(Op::Concat);
                // Continue with template parts
                self.compile_template_continuation()?;
                Ok(())
            }

            TokenKind::True => {
                self.lexer.next_token();
                let idx = self.code.add_number(1.0);
                self.code.emit_op_u16(Op::LoadConst, idx);
                Ok(())
            }

            TokenKind::False => {
                self.lexer.next_token();
                let idx = self.code.add_number(0.0);
                self.code.emit_op_u16(Op::LoadConst, idx);
                Ok(())
            }

            TokenKind::Null => {
                self.lexer.next_token();
                self.emit_null();
                Ok(())
            }

            TokenKind::Undefined => {
                self.lexer.next_token();
                self.emit_undefined();
                Ok(())
            }

            TokenKind::Identifier(name) => {
                let name = name.clone();
                self.lexer.next_token();

                // Check for print() built-in
                if name == "print" && self.lexer.peek().kind == TokenKind::LeftParen {
                    self.lexer.next_token(); // consume '('
                    let mut argc: u8 = 0;
                    if self.lexer.peek().kind != TokenKind::RightParen {
                        loop {
                            self.compile_expression()?;
                            argc += 1;
                            if !self.lexer.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    self.lexer.expect(&TokenKind::RightParen)?;
                    self.code.emit_op_u8(Op::Print, argc);
                    return Ok(());
                }

                // Regular variable lookup
                if let Some((slot, _)) = self.resolve_local(&name) {
                    self.code.emit_op_u16(Op::LoadLocal, slot);
                } else {
                    let name_id = self.strings.intern(&name);
                    let idx = self.code.add_string(name_id.0);
                    self.code.emit_op_u16(Op::LoadGlobal, idx);
                }
                Ok(())
            }

            TokenKind::LeftParen => {
                // Check if this is an arrow function: (...) =>
                if self.is_arrow_function() {
                    return self.compile_arrow_function();
                }
                self.lexer.next_token();
                self.compile_expression()?;
                self.lexer.expect(&TokenKind::RightParen)?;
                Ok(())
            }

            TokenKind::Function => {
                self.lexer.next_token(); // consume 'function'

                // Optional function name
                let name = if let TokenKind::Identifier(n) = &self.lexer.peek().kind {
                    let n = n.clone();
                    self.lexer.next_token();
                    n
                } else {
                    String::from("<anonymous>")
                };

                let func_index = self.compile_function_body(&name)?;
                let const_idx = self.code.add_constant(Constant::Function(func_index as u32));
                self.code.emit_op_u16(Op::LoadConst, const_idx);
                Ok(())
            }

            // Array literal: [expr, expr, ...]
            TokenKind::LeftBracket => {
                self.lexer.next_token(); // consume '['
                let mut count: u16 = 0;
                if self.lexer.peek().kind != TokenKind::RightBracket {
                    loop {
                        // Allow trailing comma
                        if self.lexer.peek().kind == TokenKind::RightBracket {
                            break;
                        }
                        self.compile_expression()?;
                        count += 1;
                        if !self.lexer.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.lexer.expect(&TokenKind::RightBracket)?;
                self.code.emit_op_u16(Op::CreateArray, count);
                Ok(())
            }

            // Object literal: { key: value, ... }
            TokenKind::LeftBrace => {
                self.compile_object_literal()
            }

            _ => Err(JsError::syntax(
                alloc::format!("unexpected token '{}'", tok.kind),
                tok.span.line,
                tok.span.column,
            )),
        }
    }

    fn compile_template_continuation(&mut self) -> JsResult<()> {
        // After TemplateHead + expression + Concat, the next token from
        // tokenize_all() is either TemplateTail or TemplateMiddle.
        loop {
            let tok = self.lexer.peek().clone();
            match &tok.kind {
                TokenKind::TemplateTail(s) => {
                    let s = s.clone();
                    self.lexer.next_token();
                    if !s.is_empty() {
                        let id = self.strings.intern(&s);
                        let idx = self.code.add_string(id.0);
                        self.code.emit_op_u16(Op::LoadConst, idx);
                        self.code.emit_op(Op::Concat);
                    }
                    break;
                }
                TokenKind::TemplateMiddle(s) => {
                    let s = s.clone();
                    self.lexer.next_token();
                    if !s.is_empty() {
                        let id = self.strings.intern(&s);
                        let idx = self.code.add_string(id.0);
                        self.code.emit_op_u16(Op::LoadConst, idx);
                        self.code.emit_op(Op::Concat);
                    }
                    // Compile the next expression and concatenate
                    self.compile_expression()?;
                    self.code.emit_op(Op::Concat);
                    // Continue loop to read next tail/middle
                }
                _ => {
                    return Err(JsError::syntax(
                        "unexpected token in template literal",
                        tok.span.line,
                        tok.span.column,
                    ));
                }
            }
        }
        Ok(())
    }

    fn compile_object_literal(&mut self) -> JsResult<()> {
        self.lexer.next_token(); // consume '{'
        self.code.emit_op(Op::CreateObject);

        if self.lexer.peek().kind != TokenKind::RightBrace {
            loop {
                if self.lexer.peek().kind == TokenKind::RightBrace {
                    break;
                }

                // Parse property key
                let key_tok = self.lexer.next_token().clone();
                let key_name = match &key_tok.kind {
                    TokenKind::Identifier(n) => n.clone(),
                    TokenKind::String(s) => s.clone(),
                    TokenKind::Number(n) => alloc::format!("{}", *n as i64),
                    _ if key_tok.kind.is_keyword() => alloc::format!("{}", key_tok.kind),
                    _ => {
                        return Err(JsError::syntax(
                            "expected property name",
                            key_tok.span.line,
                            key_tok.span.column,
                        ));
                    }
                };

                // Check for shorthand { name } (identifier only, no colon)
                if self.lexer.peek().kind == TokenKind::Comma
                    || self.lexer.peek().kind == TokenKind::RightBrace
                {
                    // Shorthand property: { x } means { x: x }
                    if let Some((slot, _)) = self.resolve_local(&key_name) {
                        self.code.emit_op_u16(Op::LoadLocal, slot);
                    } else {
                        let name_id = self.strings.intern(&key_name);
                        let idx = self.code.add_string(name_id.0);
                        self.code.emit_op_u16(Op::LoadGlobal, idx);
                    }
                } else {
                    self.lexer.expect(&TokenKind::Colon)?;
                    self.compile_expression()?;
                }

                let key_id = self.strings.intern(&key_name);
                let key_idx = self.code.add_string(key_id.0);
                self.code.emit_op_u16(Op::SetPropertyConst, key_idx);

                if !self.lexer.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }

        self.lexer.expect(&TokenKind::RightBrace)?;
        Ok(())
    }

    // --- Scope management ---

    fn push_scope(&mut self) {
        self.scopes.push(Scope {
            locals: Vec::new(),
            base_slot: self.next_slot,
        });
    }

    fn pop_scope(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            self.next_slot = scope.base_slot;
        }
    }

    fn declare_local(&mut self, name: String, is_const: bool) -> u16 {
        let slot = self.next_slot;
        self.next_slot += 1;
        if let Some(scope) = self.scopes.last_mut() {
            scope.locals.push(Local {
                name,
                slot,
                is_const,
            });
        }
        slot
    }

    fn resolve_local(&self, name: &str) -> Option<(u16, bool)> {
        // Search from innermost scope outward
        for scope in self.scopes.iter().rev() {
            for local in scope.locals.iter().rev() {
                if local.name == name {
                    return Some((local.slot, local.is_const));
                }
            }
        }
        None
    }

    // --- Helpers ---

    fn emit_undefined(&mut self) {
        // Emit undefined as a special constant
        // For Phase 1, we use NaN to represent undefined in the constant pool
        // The VM will handle this correctly
        let idx = self.code.add_constant(Constant::Number(f64::NAN));
        self.code.emit_op_u16(Op::LoadConst, idx);
    }

    fn emit_null(&mut self) {
        // For Phase 1, null is 0.0 (will be properly handled in Phase 2)
        let idx = self.code.add_number(0.0);
        self.code.emit_op_u16(Op::LoadConst, idx);
    }

    fn eat_semicolon(&mut self) {
        // Auto-semicolon insertion: just consume if present
        self.lexer.eat(&TokenKind::Semicolon);
    }

    /// Check if the current position is the start of an arrow function.
    /// Looks ahead past balanced parentheses for `=>`.
    fn is_arrow_function(&self) -> bool {
        // Current token must be '('
        if self.lexer.peek().kind != TokenKind::LeftParen {
            return false;
        }

        // Scan ahead to find matching ')' then check for '=>'
        let mut depth = 0;
        let mut offset = 0;
        loop {
            let tok = self.lexer.peek_ahead(offset);
            match &tok.kind {
                TokenKind::LeftParen => depth += 1,
                TokenKind::RightParen => {
                    depth -= 1;
                    if depth == 0 {
                        // Check if next token after ')' is '=>'
                        let after = self.lexer.peek_ahead(offset + 1);
                        return after.kind == TokenKind::Arrow;
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            offset += 1;
        }
    }

    /// Compile an arrow function: (params) => expr  or  (params) => { body }
    fn compile_arrow_function(&mut self) -> JsResult<()> {
        self.lexer.expect(&TokenKind::LeftParen)?;

        // Parse parameter list
        let mut params: Vec<String> = Vec::new();
        if self.lexer.peek().kind != TokenKind::RightParen {
            loop {
                let tok = self.lexer.next_token().clone();
                match &tok.kind {
                    TokenKind::Identifier(n) => params.push(n.clone()),
                    _ => {
                        return Err(JsError::syntax(
                            "expected parameter name",
                            tok.span.line,
                            tok.span.column,
                        ));
                    }
                }
                if !self.lexer.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.lexer.expect(&TokenKind::RightParen)?;
        self.lexer.expect(&TokenKind::Arrow)?;

        // Save current compiler state
        let prev_code = core::mem::replace(&mut self.code, CodeBlock::new("<arrow>"));
        let prev_scopes = core::mem::replace(
            &mut self.scopes,
            vec![Scope {
                locals: Vec::new(),
                base_slot: 0,
            }],
        );
        let prev_next_slot = self.next_slot;
        self.next_slot = 0;

        // Declare parameters as locals
        for param in &params {
            self.declare_local(param.clone(), false);
        }

        // Compile body: either a block or a single expression
        if self.lexer.peek().kind == TokenKind::LeftBrace {
            // Block body: { statements }
            self.lexer.next_token(); // consume '{'
            while self.lexer.peek().kind != TokenKind::RightBrace
                && self.lexer.peek().kind != TokenKind::Eof
            {
                self.compile_statement()?;
            }
            self.lexer.expect(&TokenKind::RightBrace)?;
            // Implicit undefined return if no explicit return
            self.emit_undefined();
            self.code.emit_op(Op::Return);
        } else {
            // Expression body: the expression result is the return value
            self.compile_assignment()?;
            self.code.emit_op(Op::Return);
        }

        self.code.local_count = self.next_slot;

        // Restore state
        let func_code = core::mem::replace(&mut self.code, prev_code);
        self.scopes = prev_scopes;
        self.next_slot = prev_next_slot;

        let func_index = self.functions.len();
        self.functions.push(func_code);

        let const_idx = self.code.add_constant(Constant::Function(func_index as u32));
        self.code.emit_op_u16(Op::LoadConst, const_idx);
        Ok(())
    }
}
