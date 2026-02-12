//! breenish-js: ECMAScript engine for the Breenish shell.
//!
//! A minimal but real JavaScript engine written in Rust, designed to be the
//! scripting language for the Breenish shell on Breenix OS.
//!
//! # Architecture
//!
//! - **Lexer** (`lexer.rs`): Single-pass tokenizer
//! - **Compiler** (`compiler.rs`): Recursive descent, direct source-to-bytecode (no AST)
//! - **VM** (`vm.rs`): Stack-based bytecode interpreter
//! - **Values** (`value.rs`): NaN-boxed 64-bit tagged values
//! - **Strings** (`string.rs`): Interned string pool
//!
//! # Usage
//!
//! ```rust
//! use breenish_js::Context;
//!
//! let mut ctx = Context::new();
//! ctx.eval("let x = 1 + 2; print(x);").unwrap();
//! // Prints: 3
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod bytecode;
pub mod compiler;
pub mod error;
pub mod lexer;
pub mod string;
pub mod token;
pub mod value;
pub mod vm;

use alloc::vec::Vec;

use compiler::Compiler;
use error::JsResult;
use string::StringPool;
use value::JsValue;
use vm::{PrintFn, Vm};
use bytecode::CodeBlock;

/// The main entry point for the breenish-js engine.
///
/// A Context holds the VM state and string pool, allowing multiple
/// evaluations to share state.
pub struct Context {
    vm: Vm,
    strings: StringPool,
}

impl Context {
    /// Create a new JavaScript execution context.
    pub fn new() -> Self {
        Self {
            vm: Vm::new(),
            strings: StringPool::new(),
        }
    }

    /// Set the print output callback.
    pub fn set_print_fn(&mut self, f: PrintFn) {
        self.vm.set_print_fn(f);
    }

    /// Evaluate a JavaScript source string.
    pub fn eval(&mut self, source: &str) -> JsResult<JsValue> {
        let compiler = Compiler::new(source);
        let (code, mut compile_strings, functions) = compiler.compile()?;

        // Merge compiled strings into our persistent pool
        // For Phase 1, we just use the compiler's string pool directly
        // since we create a new compiler each time
        self.vm.execute(&code, &mut compile_strings, &functions)
    }

    /// Compile source to bytecode without executing.
    #[cfg(feature = "std")]
    pub fn compile(&self, source: &str) -> JsResult<(CodeBlock, StringPool, Vec<CodeBlock>)> {
        let compiler = Compiler::new(source);
        compiler.compile()
    }

    /// Get a reference to the string pool.
    pub fn strings(&self) -> &StringPool {
        &self.strings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use core::cell::RefCell;

    // Thread-local buffer to capture print output in tests.
    thread_local! {
        static OUTPUT: RefCell<String> = RefCell::new(String::new());
    }

    fn capture_print(s: &str) {
        OUTPUT.with(|o| o.borrow_mut().push_str(s));
    }

    fn take_output() -> String {
        OUTPUT.with(|o| {
            let s = o.borrow().clone();
            o.borrow_mut().clear();
            s
        })
    }

    fn eval_and_capture(source: &str) -> String {
        let mut ctx = Context::new();
        ctx.set_print_fn(capture_print);
        ctx.eval(source).unwrap();
        take_output()
    }

    #[test]
    fn test_basic_arithmetic() {
        assert_eq!(eval_and_capture("print(1 + 2);"), "3\n");
        assert_eq!(eval_and_capture("print(10 - 3);"), "7\n");
        assert_eq!(eval_and_capture("print(6 * 7);"), "42\n");
        assert_eq!(eval_and_capture("print(15 / 4);"), "3.75\n");
        assert_eq!(eval_and_capture("print(17 % 5);"), "2\n");
    }

    #[test]
    fn test_let_variable() {
        assert_eq!(eval_and_capture("let x = 1 + 2; print(x);"), "3\n");
    }

    #[test]
    fn test_multiple_variables() {
        assert_eq!(
            eval_and_capture("let a = 10; let b = 20; print(a + b);"),
            "30\n"
        );
    }

    #[test]
    fn test_string_literal() {
        assert_eq!(eval_and_capture("print(\"hello\");"), "hello\n");
    }

    #[test]
    fn test_string_concatenation() {
        assert_eq!(
            eval_and_capture("print(\"hello\" + \" \" + \"world\");"),
            "hello world\n"
        );
    }

    #[test]
    fn test_if_else() {
        assert_eq!(eval_and_capture("if (1) { print(\"yes\"); }"), "yes\n");
        assert_eq!(
            eval_and_capture("if (0) { print(\"yes\"); } else { print(\"no\"); }"),
            "no\n"
        );
    }

    #[test]
    fn test_while_loop() {
        assert_eq!(
            eval_and_capture("let i = 0; while (i < 5) { i = i + 1; } print(i);"),
            "5\n"
        );
    }

    #[test]
    fn test_for_loop() {
        assert_eq!(
            eval_and_capture("let sum = 0; for (let i = 1; i <= 10; i = i + 1) { sum = sum + i; } print(sum);"),
            "55\n"
        );
    }

    #[test]
    fn test_function_declaration() {
        assert_eq!(
            eval_and_capture("function add(a, b) { return a + b; } print(add(3, 4));"),
            "7\n"
        );
    }

    #[test]
    fn test_recursive_function() {
        assert_eq!(
            eval_and_capture(
                "function fib(n) { if (n <= 1) return n; return fib(n - 1) + fib(n - 2); } print(fib(10));"
            ),
            "55\n"
        );
    }

    #[test]
    fn test_nested_function_calls() {
        assert_eq!(
            eval_and_capture(
                "function double(x) { return x * 2; } function triple(x) { return x * 3; } print(double(triple(5)));"
            ),
            "30\n"
        );
    }

    #[test]
    fn test_comparison_operators() {
        assert_eq!(eval_and_capture("print(1 < 2);"), "1\n");
        assert_eq!(eval_and_capture("print(2 > 3);"), "0\n");
        assert_eq!(eval_and_capture("print(5 <= 5);"), "1\n");
        assert_eq!(eval_and_capture("print(5 >= 6);"), "0\n");
    }

    #[test]
    fn test_logical_not() {
        assert_eq!(eval_and_capture("print(!0);"), "1\n");
        assert_eq!(eval_and_capture("print(!1);"), "0\n");
    }

    #[test]
    fn test_unary_negate() {
        assert_eq!(eval_and_capture("print(-5);"), "-5\n");
        assert_eq!(eval_and_capture("let x = 3; print(-x);"), "-3\n");
    }

    #[test]
    fn test_multiple_print_args() {
        assert_eq!(eval_and_capture("print(1, 2, 3);"), "1 2 3\n");
    }

    #[test]
    fn test_break_in_loop() {
        assert_eq!(
            eval_and_capture("let i = 0; while (1) { if (i === 3) break; i = i + 1; } print(i);"),
            "3\n"
        );
    }

    #[test]
    fn test_template_literal_no_sub() {
        assert_eq!(eval_and_capture("print(`hello world`);"), "hello world\n");
    }

    #[test]
    fn test_compound_assignment() {
        assert_eq!(
            eval_and_capture("let x = 10; x += 5; print(x);"),
            "15\n"
        );
    }

    #[test]
    fn test_ternary() {
        assert_eq!(eval_and_capture("print(1 ? \"yes\" : \"no\");"), "yes\n");
        assert_eq!(eval_and_capture("print(0 ? \"yes\" : \"no\");"), "no\n");
    }
}
