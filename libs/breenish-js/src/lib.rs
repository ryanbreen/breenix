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
pub mod object;
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

// Re-export types needed by native function implementors.
pub use vm::NativeFn;

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

    /// Register a native function that can be called from JavaScript.
    ///
    /// Must be called before any `eval()` calls that reference the function.
    pub fn register_native(&mut self, name: &str, func: NativeFn) {
        self.vm.register_native(name, func);
    }

    /// Register built-in Promise support.
    ///
    /// This registers Promise.resolve, Promise.reject, Promise.all as native
    /// functions and creates a Promise global object.
    pub fn register_promise_builtins(&mut self) {
        use crate::object::{JsObject, ObjectHeap, ObjectKind, PromiseState};
        use crate::string::StringPool as SP;
        use crate::value::JsValue;
        use crate::error::{JsError, JsResult};

        fn promise_resolve(args: &[JsValue], _strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            let obj = JsObject::new_promise_fulfilled(val);
            let idx = heap.alloc(obj);
            Ok(JsValue::object(idx))
        }

        fn promise_reject(args: &[JsValue], _strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            let obj = JsObject::new_promise_rejected(val);
            let idx = heap.alloc(obj);
            Ok(JsValue::object(idx))
        }

        fn promise_all(args: &[JsValue], _strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let arr_val = args.first().copied().unwrap_or(JsValue::undefined());
            if !arr_val.is_object() {
                return Err(JsError::type_error("Promise.all: expected array"));
            }

            let arr_idx = arr_val.as_object_index();
            let elements: Vec<JsValue> = {
                let arr = heap.get(arr_idx)
                    .ok_or_else(|| JsError::type_error("Promise.all: invalid array"))?;
                arr.elements().to_vec()
            };

            let mut results = Vec::new();
            for elem in elements {
                if elem.is_object() {
                    let obj = heap.get(elem.as_object_index());
                    match obj {
                        Some(obj) => match &obj.kind {
                            ObjectKind::Promise(PromiseState::Fulfilled(v)) => results.push(*v),
                            ObjectKind::Promise(PromiseState::Rejected(v)) => {
                                let rej = JsObject::new_promise_rejected(*v);
                                let idx = heap.alloc(rej);
                                return Ok(JsValue::object(idx));
                            }
                            ObjectKind::Promise(PromiseState::Pending) => {
                                return Err(JsError::runtime("Promise.all: unresolved pending promise"));
                            }
                            _ => results.push(elem),
                        },
                        None => results.push(elem),
                    }
                } else {
                    results.push(elem);
                }
            }

            let mut result_arr = JsObject::new_array();
            for val in &results {
                result_arr.push(*val);
            }
            let arr_obj_idx = heap.alloc(result_arr);

            let promise = JsObject::new_promise_fulfilled(JsValue::object(arr_obj_idx));
            let promise_idx = heap.alloc(promise);
            Ok(JsValue::object(promise_idx))
        }

        fn promise_race(args: &[JsValue], _strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let arr_val = args.first().copied().unwrap_or(JsValue::undefined());
            if !arr_val.is_object() {
                return Err(JsError::type_error("Promise.race: expected array"));
            }

            let arr_idx = arr_val.as_object_index();
            let elements: Vec<JsValue> = {
                let arr = heap.get(arr_idx)
                    .ok_or_else(|| JsError::type_error("Promise.race: invalid array"))?;
                arr.elements().to_vec()
            };

            // Return the first fulfilled promise's value, or the first rejected
            // if none are fulfilled (synchronous model).
            let mut first_rejected: Option<JsValue> = None;
            for elem in &elements {
                if elem.is_object() {
                    if let Some(obj) = heap.get(elem.as_object_index()) {
                        match &obj.kind {
                            ObjectKind::Promise(PromiseState::Fulfilled(v)) => {
                                let result = JsObject::new_promise_fulfilled(*v);
                                let idx = heap.alloc(result);
                                return Ok(JsValue::object(idx));
                            }
                            ObjectKind::Promise(PromiseState::Rejected(v)) => {
                                if first_rejected.is_none() {
                                    first_rejected = Some(*v);
                                }
                            }
                            ObjectKind::Promise(PromiseState::Pending) => {
                                return Err(JsError::runtime("Promise.race: unresolved pending promise"));
                            }
                            _ => {
                                // Non-promise object treated as fulfilled value
                                let result = JsObject::new_promise_fulfilled(*elem);
                                let idx = heap.alloc(result);
                                return Ok(JsValue::object(idx));
                            }
                        }
                    }
                } else {
                    // Non-object value treated as fulfilled
                    let result = JsObject::new_promise_fulfilled(*elem);
                    let idx = heap.alloc(result);
                    return Ok(JsValue::object(idx));
                }
            }

            // All rejected (or empty array) - return the first rejection
            if let Some(reason) = first_rejected {
                let result = JsObject::new_promise_rejected(reason);
                let idx = heap.alloc(result);
                Ok(JsValue::object(idx))
            } else {
                // Empty array: return a pending promise (per spec, never resolves)
                let result = JsObject::new_promise_pending();
                let idx = heap.alloc(result);
                Ok(JsValue::object(idx))
            }
        }

        fn promise_all_settled(args: &[JsValue], strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let arr_val = args.first().copied().unwrap_or(JsValue::undefined());
            if !arr_val.is_object() {
                return Err(JsError::type_error("Promise.allSettled: expected array"));
            }

            let arr_idx = arr_val.as_object_index();
            let elements: Vec<JsValue> = {
                let arr = heap.get(arr_idx)
                    .ok_or_else(|| JsError::type_error("Promise.allSettled: invalid array"))?;
                arr.elements().to_vec()
            };

            let status_key = strings.intern("status");
            let fulfilled_str = strings.intern("fulfilled");
            let rejected_str = strings.intern("rejected");
            let value_key = strings.intern("value");
            let reason_key = strings.intern("reason");

            let mut result_indices: Vec<u32> = Vec::new();
            for elem in &elements {
                if elem.is_object() {
                    if let Some(obj) = heap.get(elem.as_object_index()) {
                        match &obj.kind {
                            ObjectKind::Promise(PromiseState::Fulfilled(v)) => {
                                let v_copy = *v;
                                let mut entry = JsObject::new();
                                entry.set(status_key, JsValue::string(fulfilled_str));
                                entry.set(value_key, v_copy);
                                result_indices.push(heap.alloc(entry));
                            }
                            ObjectKind::Promise(PromiseState::Rejected(v)) => {
                                let v_copy = *v;
                                let mut entry = JsObject::new();
                                entry.set(status_key, JsValue::string(rejected_str));
                                entry.set(reason_key, v_copy);
                                result_indices.push(heap.alloc(entry));
                            }
                            ObjectKind::Promise(PromiseState::Pending) => {
                                return Err(JsError::runtime("Promise.allSettled: unresolved pending promise"));
                            }
                            _ => {
                                // Non-promise object: treat as fulfilled
                                let mut entry = JsObject::new();
                                entry.set(status_key, JsValue::string(fulfilled_str));
                                entry.set(value_key, *elem);
                                result_indices.push(heap.alloc(entry));
                            }
                        }
                    }
                } else {
                    // Non-object: treat as fulfilled with the value itself
                    let mut entry = JsObject::new();
                    entry.set(status_key, JsValue::string(fulfilled_str));
                    entry.set(value_key, *elem);
                    result_indices.push(heap.alloc(entry));
                }
            }

            // Build the result array
            let mut result_arr = JsObject::new_array();
            for idx in &result_indices {
                result_arr.push(JsValue::object(*idx));
            }
            let arr_obj_idx = heap.alloc(result_arr);

            let promise = JsObject::new_promise_fulfilled(JsValue::object(arr_obj_idx));
            let promise_idx = heap.alloc(promise);
            Ok(JsValue::object(promise_idx))
        }

        // Register the native functions
        self.vm.register_native("Promise_resolve", promise_resolve);
        self.vm.register_native("Promise_reject", promise_reject);
        self.vm.register_native("Promise_all", promise_all);
        self.vm.register_native("Promise_race", promise_race);
        self.vm.register_native("Promise_allSettled", promise_all_settled);

        // Build the Promise global object programmatically so it persists
        // across eval() calls (unlike `let` which creates a local).
        let resolve_idx = self.vm.native_obj_indices[self.vm.native_obj_indices.len() - 5];
        let reject_idx = self.vm.native_obj_indices[self.vm.native_obj_indices.len() - 4];
        let all_idx = self.vm.native_obj_indices[self.vm.native_obj_indices.len() - 3];
        let race_idx = self.vm.native_obj_indices[self.vm.native_obj_indices.len() - 2];
        let all_settled_idx = self.vm.native_obj_indices[self.vm.native_obj_indices.len() - 1];

        let mut promise_obj = JsObject::new();
        let resolve_key = self.strings.intern("resolve");
        let reject_key = self.strings.intern("reject");
        let all_key = self.strings.intern("all");
        let race_key = self.strings.intern("race");
        let all_settled_key = self.strings.intern("allSettled");
        promise_obj.set(resolve_key, JsValue::object(resolve_idx));
        promise_obj.set(reject_key, JsValue::object(reject_idx));
        promise_obj.set(all_key, JsValue::object(all_idx));
        promise_obj.set(race_key, JsValue::object(race_idx));
        promise_obj.set(all_settled_key, JsValue::object(all_settled_idx));

        let obj_idx = self.vm.heap.alloc(promise_obj);
        self.vm.set_global_by_name("Promise", JsValue::object(obj_idx), &mut self.strings);
    }

    /// Get a mutable reference to the string pool (for native functions).
    pub fn strings_mut(&mut self) -> &mut StringPool {
        &mut self.strings
    }

    /// Evaluate a JavaScript source string.
    pub fn eval(&mut self, source: &str) -> JsResult<JsValue> {
        let compiler = Compiler::new(source);
        let (code, mut compile_strings, functions) = compiler.compile()?;

        // Re-register native function globals and persistent globals using
        // the compiler's string pool so that lookups match correct string IDs.
        self.vm.sync_natives(&self.strings, &mut compile_strings);

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

    // --- Phase 2 tests: Objects, Arrays, Property Access ---

    #[test]
    fn test_array_literal() {
        assert_eq!(eval_and_capture("let a = [1, 2, 3]; print(a);"), "1,2,3\n");
    }

    #[test]
    fn test_array_index() {
        assert_eq!(
            eval_and_capture("let a = [10, 20, 30]; print(a[1]);"),
            "20\n"
        );
    }

    #[test]
    fn test_array_length() {
        assert_eq!(
            eval_and_capture("let a = [1, 2, 3, 4, 5]; print(a.length);"),
            "5\n"
        );
    }

    #[test]
    fn test_object_literal() {
        assert_eq!(
            eval_and_capture("let o = { x: 10, y: 20 }; print(o.x + o.y);"),
            "30\n"
        );
    }

    #[test]
    fn test_object_property_set() {
        assert_eq!(
            eval_and_capture("let o = {}; o.name = \"breenish\"; print(o.name);"),
            "breenish\n"
        );
    }

    #[test]
    fn test_object_bracket_access() {
        assert_eq!(
            eval_and_capture("let o = { hello: 42 }; let k = \"hello\"; print(o[k]);"),
            "42\n"
        );
    }

    #[test]
    fn test_nested_objects() {
        assert_eq!(
            eval_and_capture("let o = { inner: { value: 99 } }; print(o.inner.value);"),
            "99\n"
        );
    }

    #[test]
    fn test_array_in_loop() {
        assert_eq!(
            eval_and_capture(
                "let a = [10, 20, 30]; let sum = 0; for (let i = 0; i < a.length; i = i + 1) { sum = sum + a[i]; } print(sum);"
            ),
            "60\n"
        );
    }

    #[test]
    fn test_object_with_function() {
        assert_eq!(
            eval_and_capture(
                "let o = { x: 5 }; function getX(obj) { return obj.x; } print(getX(o));"
            ),
            "5\n"
        );
    }

    #[test]
    fn test_empty_array() {
        assert_eq!(
            eval_and_capture("let a = []; print(a.length);"),
            "0\n"
        );
    }

    #[test]
    fn test_string_length() {
        assert_eq!(
            eval_and_capture("let s = \"hello\"; print(s.length);"),
            "5\n"
        );
    }

    // --- Arrow function tests ---

    #[test]
    fn test_arrow_function_expression_body() {
        assert_eq!(
            eval_and_capture("let double = (x) => x * 2; print(double(5));"),
            "10\n"
        );
    }

    #[test]
    fn test_arrow_function_block_body() {
        assert_eq!(
            eval_and_capture("let add = (a, b) => { return a + b; }; print(add(3, 4));"),
            "7\n"
        );
    }

    #[test]
    fn test_arrow_function_no_params() {
        assert_eq!(
            eval_and_capture("let greet = () => \"hello\"; print(greet());"),
            "hello\n"
        );
    }

    #[test]
    fn test_arrow_function_in_variable() {
        assert_eq!(
            eval_and_capture("let nums = [1, 2, 3]; let f = (x) => x + 10; print(f(nums[1]));"),
            "12\n"
        );
    }

    #[test]
    fn test_arrow_function_passed_to_function() {
        assert_eq!(
            eval_and_capture(
                "function apply(f, x) { return f(x); } print(apply((n) => n * n, 7));"
            ),
            "49\n"
        );
    }

    // --- Array method tests ---

    #[test]
    fn test_array_push() {
        assert_eq!(
            eval_and_capture("let a = [1, 2]; a.push(3); print(a.length, a[2]);"),
            "3 3\n"
        );
    }

    #[test]
    fn test_array_pop() {
        assert_eq!(
            eval_and_capture("let a = [1, 2, 3]; let x = a.pop(); print(x, a.length);"),
            "3 2\n"
        );
    }

    #[test]
    fn test_array_indexof() {
        assert_eq!(
            eval_and_capture("let a = [10, 20, 30]; print(a.indexOf(20), a.indexOf(99));"),
            "1 -1\n"
        );
    }

    #[test]
    fn test_array_join() {
        assert_eq!(
            eval_and_capture("let a = [1, 2, 3]; print(a.join(\"-\"));"),
            "1-2-3\n"
        );
    }

    #[test]
    fn test_array_includes() {
        assert_eq!(
            eval_and_capture("let a = [1, 2, 3]; print(a.includes(2), a.includes(5));"),
            "1 0\n"
        );
    }

    #[test]
    fn test_array_slice() {
        assert_eq!(
            eval_and_capture("let a = [1, 2, 3, 4, 5]; let b = a.slice(1, 3); print(b);"),
            "2,3\n"
        );
    }

    #[test]
    fn test_array_concat() {
        assert_eq!(
            eval_and_capture("let a = [1, 2]; let b = [3, 4]; let c = a.concat(b); print(c);"),
            "1,2,3,4\n"
        );
    }

    // --- String method tests ---

    #[test]
    fn test_string_indexof() {
        assert_eq!(
            eval_and_capture("print(\"hello world\".indexOf(\"world\"));"),
            "6\n"
        );
    }

    #[test]
    fn test_string_includes() {
        assert_eq!(
            eval_and_capture("print(\"hello\".includes(\"ell\"), \"hello\".includes(\"xyz\"));"),
            "1 0\n"
        );
    }

    #[test]
    fn test_string_startswith_endswith() {
        assert_eq!(
            eval_and_capture("print(\"hello.rs\".startsWith(\"hello\"), \"hello.rs\".endsWith(\".rs\"));"),
            "1 1\n"
        );
    }

    #[test]
    fn test_string_trim() {
        assert_eq!(
            eval_and_capture("print(\"  hello  \".trim());"),
            "hello\n"
        );
    }

    #[test]
    fn test_string_touppercase() {
        assert_eq!(
            eval_and_capture("print(\"hello\".toUpperCase());"),
            "HELLO\n"
        );
    }

    #[test]
    fn test_string_slice() {
        assert_eq!(
            eval_and_capture("print(\"hello world\".slice(0, 5));"),
            "hello\n"
        );
    }

    #[test]
    fn test_string_split() {
        assert_eq!(
            eval_and_capture("let parts = \"a,b,c\".split(\",\"); print(parts.length, parts[1]);"),
            "3 b\n"
        );
    }

    #[test]
    fn test_string_replace() {
        assert_eq!(
            eval_and_capture("print(\"hello world\".replace(\"world\", \"breenix\"));"),
            "hello breenix\n"
        );
    }

    // --- Switch/case tests ---

    #[test]
    fn test_switch_case() {
        assert_eq!(
            eval_and_capture(
                "let x = 2; switch (x) { case 1: print(\"one\"); break; case 2: print(\"two\"); break; case 3: print(\"three\"); break; }"
            ),
            "two\n"
        );
    }

    #[test]
    fn test_switch_default() {
        assert_eq!(
            eval_and_capture(
                "let x = 99; switch (x) { case 1: print(\"one\"); break; default: print(\"other\"); break; }"
            ),
            "other\n"
        );
    }

    #[test]
    fn test_switch_fallthrough() {
        assert_eq!(
            eval_and_capture(
                "let x = 1; let r = \"\"; switch (x) { case 1: r += \"a\"; case 2: r += \"b\"; break; case 3: r += \"c\"; break; } print(r);"
            ),
            "ab\n"
        );
    }

    // --- for...of tests ---

    #[test]
    fn test_for_of_array() {
        assert_eq!(
            eval_and_capture(
                "let sum = 0; for (let x of [10, 20, 30]) { sum += x; } print(sum);"
            ),
            "60\n"
        );
    }

    #[test]
    fn test_for_of_with_break() {
        assert_eq!(
            eval_and_capture(
                "let result = \"\"; for (let x of [\"a\", \"b\", \"c\", \"d\"]) { if (x === \"c\") break; result += x; } print(result);"
            ),
            "ab\n"
        );
    }

    // --- Logical operators ---

    #[test]
    fn test_logical_and() {
        assert_eq!(eval_and_capture("print(1 && 2);"), "2\n");
        assert_eq!(eval_and_capture("print(0 && 2);"), "0\n");
    }

    #[test]
    fn test_logical_or() {
        assert_eq!(eval_and_capture("print(0 || 2);"), "2\n");
        assert_eq!(eval_and_capture("print(1 || 2);"), "1\n");
    }

    // --- Template literals with interpolation ---

    #[test]
    fn test_template_literal_interpolation() {
        assert_eq!(
            eval_and_capture("let name = \"world\"; print(`hello ${name}`);"),
            "hello world\n"
        );
    }

    #[test]
    fn test_template_literal_expression() {
        assert_eq!(
            eval_and_capture("print(`2 + 3 = ${2 + 3}`);"),
            "2 + 3 = 5\n"
        );
    }

    // --- Closure tests ---

    #[test]
    fn test_closure_basic_capture() {
        assert_eq!(
            eval_and_capture(
                "function makeAdder(x) { return (y) => x + y; } let add5 = makeAdder(5); print(add5(3));"
            ),
            "8\n"
        );
    }

    #[test]
    fn test_closure_counter() {
        assert_eq!(
            eval_and_capture(
                "function makeCounter() { let count = 0; return () => { count += 1; return count; }; } let c = makeCounter(); print(c(), c(), c());"
            ),
            "1 2 3\n"
        );
    }

    #[test]
    fn test_closure_preserves_environment() {
        assert_eq!(
            eval_and_capture(
                "function outer(x) { function inner() { return x * 2; } return inner(); } print(outer(21));"
            ),
            "42\n"
        );
    }

    #[test]
    fn test_closure_arrow_capture() {
        assert_eq!(
            eval_and_capture(
                "function make() { let val = 10; return () => val; } print(make()());"
            ),
            "10\n"
        );
    }

    #[test]
    fn test_closure_multiple_captures() {
        assert_eq!(
            eval_and_capture(
                "function f(a, b) { return () => a + b; } let g = f(3, 4); print(g());"
            ),
            "7\n"
        );
    }

    // --- GC tests ---

    #[test]
    fn test_gc_frees_unreachable_objects() {
        let mut ctx = Context::new();
        ctx.set_print_fn(capture_print);
        // Create objects then clear all VM state so everything is unreachable
        ctx.eval("let x = { a: 1 }; let y = [1, 2, 3];").unwrap();
        assert!(ctx.vm.heap.live_count() > 0, "should have allocated objects");
        // Clear roots manually so all objects become unreachable
        ctx.vm.clear_roots();
        ctx.vm.gc();
        assert_eq!(ctx.vm.heap.live_count(), 0, "GC should free all objects when no roots exist");
    }

    #[test]
    fn test_gc_preserves_stack_roots() {
        let mut ctx = Context::new();
        ctx.set_print_fn(capture_print);
        // After eval, Halt pops one value but leaves remaining locals on the stack.
        // Those remaining locals should be preserved by GC.
        ctx.eval("let x = { a: 1 }; let y = [1, 2, 3];").unwrap();
        let before = ctx.vm.heap.live_count();
        ctx.vm.gc();
        let after = ctx.vm.heap.live_count();
        // At least one object should survive (whatever is still on the stack)
        assert!(after > 0, "GC should preserve objects still on the stack");
        assert!(after <= before, "GC should not create objects");
    }

    #[test]
    fn test_gc_heap_mark_sweep_mechanics() {
        use crate::object::{JsObject, ObjectHeap};
        // Test the mark/sweep mechanics directly
        let mut heap = ObjectHeap::new();
        let a = heap.alloc(JsObject::new());
        let b = heap.alloc(JsObject::new());
        let c = heap.alloc(JsObject::new());
        assert_eq!(heap.live_count(), 3);

        // Mark only 'a', sweep should free b and c
        heap.unmark_all();
        heap.mark(a);
        let freed = heap.sweep();
        assert_eq!(freed, 2);
        assert_eq!(heap.live_count(), 1);
        assert!(heap.get(a).is_some());
        assert!(heap.get(b).is_none());
        assert!(heap.get(c).is_none());
    }

    #[test]
    fn test_gc_mark_traces_object_graph() {
        use crate::object::{JsObject, ObjectHeap};
        use crate::string::StringPool;
        // Create an object graph: root -> child -> grandchild
        let mut heap = ObjectHeap::new();
        let mut strings = StringPool::new();

        let grandchild = heap.alloc(JsObject::new());
        let child_idx = {
            let mut child = JsObject::new();
            let key = strings.intern("gc");
            child.set(key, JsValue::object(grandchild));
            heap.alloc(child)
        };
        let root = {
            let mut root_obj = JsObject::new();
            let key = strings.intern("child");
            root_obj.set(key, JsValue::object(child_idx));
            heap.alloc(root_obj)
        };

        // Mark only root - child and grandchild should also be marked via tracing
        heap.unmark_all();
        heap.mark(root);
        let freed = heap.sweep();
        assert_eq!(freed, 0, "all objects reachable from root should survive");
        assert_eq!(heap.live_count(), 3);
    }

    // --- Try/catch/finally tests ---

    #[test]
    fn test_try_catch_basic() {
        assert_eq!(
            eval_and_capture(
                "try { throw \"oops\"; } catch (e) { print(e); }"
            ),
            "oops\n"
        );
    }

    #[test]
    fn test_try_catch_no_error() {
        assert_eq!(
            eval_and_capture(
                "try { print(\"ok\"); } catch (e) { print(\"caught\"); }"
            ),
            "ok\n"
        );
    }

    #[test]
    fn test_try_catch_finally() {
        assert_eq!(
            eval_and_capture(
                "try { throw \"err\"; } catch (e) { print(e); } finally { print(\"done\"); }"
            ),
            "err\ndone\n"
        );
    }

    #[test]
    fn test_try_finally_no_error() {
        assert_eq!(
            eval_and_capture(
                "try { print(\"ok\"); } catch (e) { print(\"caught\"); } finally { print(\"fin\"); }"
            ),
            "ok\nfin\n"
        );
    }

    #[test]
    fn test_try_catch_runtime_error() {
        // TypeError from calling a non-function should be caught by try/catch
        assert_eq!(
            eval_and_capture(
                "let result = \"no error\"; try { let x = 5; x(); } catch (e) { result = \"caught\"; } print(result);"
            ),
            "caught\n"
        );
    }

    #[test]
    fn test_try_catch_throw_number() {
        assert_eq!(
            eval_and_capture(
                "try { throw 42; } catch (e) { print(e); }"
            ),
            "42\n"
        );
    }

    #[test]
    fn test_try_catch_nested() {
        assert_eq!(
            eval_and_capture(
                "try { try { throw \"inner\"; } catch (e) { print(e); } throw \"outer\"; } catch (e) { print(e); }"
            ),
            "inner\nouter\n"
        );
    }

    #[test]
    fn test_try_catch_in_function() {
        assert_eq!(
            eval_and_capture(
                "function safe(f) { try { return f(); } catch (e) { return \"error: \" + e; } } function bad() { throw \"boom\"; } print(safe(bad));"
            ),
            "error: boom\n"
        );
    }

    // --- Destructuring tests ---

    #[test]
    fn test_object_destructuring_basic() {
        assert_eq!(
            eval_and_capture(
                "let o = { x: 10, y: 20 }; let { x, y } = o; print(x, y);"
            ),
            "10 20\n"
        );
    }

    #[test]
    fn test_object_destructuring_renamed() {
        assert_eq!(
            eval_and_capture(
                "let o = { name: \"breenish\", version: 1 }; let { name: n, version: v } = o; print(n, v);"
            ),
            "breenish 1\n"
        );
    }

    #[test]
    fn test_object_destructuring_from_function() {
        assert_eq!(
            eval_and_capture(
                "function getPoint() { return { x: 3, y: 4 }; } let { x, y } = getPoint(); print(x + y);"
            ),
            "7\n"
        );
    }

    #[test]
    fn test_array_destructuring_basic() {
        assert_eq!(
            eval_and_capture(
                "let [a, b, c] = [10, 20, 30]; print(a, b, c);"
            ),
            "10 20 30\n"
        );
    }

    #[test]
    fn test_array_destructuring_from_split() {
        assert_eq!(
            eval_and_capture(
                "let [first, second] = \"hello world\".split(\" \"); print(first, second);"
            ),
            "hello world\n"
        );
    }

    // --- Spread operator tests ---

    #[test]
    fn test_spread_call() {
        assert_eq!(
            eval_and_capture(
                "function add(a, b, c) { return a + b + c; } let args = [1, 2, 3]; print(add(...args));"
            ),
            "6\n"
        );
    }

    #[test]
    fn test_spread_call_with_function() {
        assert_eq!(
            eval_and_capture(
                "function greet(first, last) { return \"Hello \" + first + \" \" + last; } let names = [\"Breen\", \"ix\"]; print(greet(...names));"
            ),
            "Hello Breen ix\n"
        );
    }

    // --- Native function tests ---

    #[test]
    fn test_native_function_basic() {
        use crate::object::ObjectHeap;
        use crate::string::StringPool;
        use crate::value::JsValue;
        use crate::error::JsResult;

        fn my_add(args: &[JsValue], _strings: &mut StringPool, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let a = args.get(0).copied().unwrap_or(JsValue::undefined()).to_number();
            let b = args.get(1).copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(a + b))
        }

        let mut ctx = Context::new();
        ctx.set_print_fn(capture_print);
        ctx.register_native("nativeAdd", my_add);
        ctx.eval("print(nativeAdd(10, 20));").unwrap();
        assert_eq!(take_output(), "30\n");
    }

    #[test]
    fn test_native_function_returns_string() {
        use crate::object::ObjectHeap;
        use crate::string::StringPool;
        use crate::value::JsValue;
        use crate::error::JsResult;

        fn get_greeting(_args: &[JsValue], strings: &mut StringPool, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let id = strings.intern("hello from native");
            Ok(JsValue::string(id))
        }

        let mut ctx = Context::new();
        ctx.set_print_fn(capture_print);
        ctx.register_native("getGreeting", get_greeting);
        ctx.eval("print(getGreeting());").unwrap();
        assert_eq!(take_output(), "hello from native\n");
    }

    #[test]
    fn test_native_function_returns_object() {
        use crate::object::{JsObject, ObjectHeap};
        use crate::string::StringPool;
        use crate::value::JsValue;
        use crate::error::JsResult;

        fn make_point(args: &[JsValue], strings: &mut StringPool, heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let x = args.get(0).copied().unwrap_or(JsValue::number(0.0));
            let y = args.get(1).copied().unwrap_or(JsValue::number(0.0));
            let mut obj = JsObject::new();
            let x_key = strings.intern("x");
            let y_key = strings.intern("y");
            obj.set(x_key, x);
            obj.set(y_key, y);
            let idx = heap.alloc(obj);
            Ok(JsValue::object(idx))
        }

        let mut ctx = Context::new();
        ctx.set_print_fn(capture_print);
        ctx.register_native("makePoint", make_point);
        ctx.eval("let p = makePoint(3, 4); print(p.x + p.y);").unwrap();
        assert_eq!(take_output(), "7\n");
    }

    // --- Promise tests ---

    fn eval_promise(source: &str) -> String {
        let mut ctx = Context::new();
        ctx.set_print_fn(capture_print);
        ctx.register_promise_builtins();
        ctx.eval(source).unwrap();
        take_output()
    }

    #[test]
    fn test_promise_resolve() {
        assert_eq!(
            eval_promise("let p = Promise.resolve(42); let val = await p; print(val);"),
            "42\n"
        );
    }

    #[test]
    fn test_promise_reject_caught() {
        assert_eq!(
            eval_promise(
                "let p = Promise.reject(\"oops\"); try { await p; } catch (e) { print(e); }"
            ),
            "oops\n"
        );
    }

    #[test]
    fn test_await_non_promise() {
        assert_eq!(
            eval_promise("let x = await 42; print(x);"),
            "42\n"
        );
    }

    #[test]
    fn test_promise_all_fulfilled() {
        assert_eq!(
            eval_promise(
                "let a = Promise.resolve(1); let b = Promise.resolve(2); let c = Promise.resolve(3); let result = await Promise.all([a, b, c]); print(result[0], result[1], result[2]);"
            ),
            "1 2 3\n"
        );
    }

    #[test]
    fn test_promise_all_with_rejection() {
        assert_eq!(
            eval_promise(
                "let a = Promise.resolve(1); let b = Promise.reject(\"fail\"); try { await Promise.all([a, b]); } catch (e) { print(e); }"
            ),
            "fail\n"
        );
    }

    #[test]
    fn test_promise_then_passthrough() {
        // .then() on a fulfilled promise returns a new fulfilled promise
        assert_eq!(
            eval_promise(
                "let p = Promise.resolve(10); let p2 = p.then(() => 20); let val = await p2; print(val);"
            ),
            "10\n"  // Our simplified .then() passes through the value
        );
    }

    #[test]
    fn test_promise_catch_on_fulfilled() {
        // .catch() on a fulfilled promise is a no-op
        assert_eq!(
            eval_promise(
                "let p = Promise.resolve(99); let p2 = p.catch(() => 0); let val = await p2; print(val);"
            ),
            "99\n"
        );
    }

    #[test]
    fn test_await_in_expression() {
        assert_eq!(
            eval_promise(
                "let a = await Promise.resolve(10); let b = await Promise.resolve(20); print(a + b);"
            ),
            "30\n"
        );
    }

    #[test]
    fn test_promise_race() {
        // race returns the first fulfilled promise's value
        assert_eq!(
            eval_promise(
                "let result = await Promise.race([Promise.resolve(1), Promise.resolve(2)]); print(result);"
            ),
            "1\n"
        );
    }

    #[test]
    fn test_promise_race_with_rejection() {
        // If the first element is rejected but second is fulfilled, return the fulfilled one
        assert_eq!(
            eval_promise(
                "let result = await Promise.race([Promise.reject(\"err\"), Promise.resolve(42)]); print(result);"
            ),
            "42\n"
        );
    }

    #[test]
    fn test_promise_race_all_rejected() {
        // If all are rejected, return the first rejection reason
        assert_eq!(
            eval_promise(
                "try { await Promise.race([Promise.reject(\"first\"), Promise.reject(\"second\")]); } catch (e) { print(e); }"
            ),
            "first\n"
        );
    }

    #[test]
    fn test_promise_all_settled() {
        // allSettled returns status objects for all promises
        assert_eq!(
            eval_promise(
                "let results = await Promise.allSettled([Promise.resolve(1), Promise.reject(\"err\"), Promise.resolve(3)]); print(results[0].status, results[0].value); print(results[1].status, results[1].reason); print(results[2].status, results[2].value);"
            ),
            "fulfilled 1\nrejected err\nfulfilled 3\n"
        );
    }

    #[test]
    fn test_promise_all_settled_all_fulfilled() {
        assert_eq!(
            eval_promise(
                "let results = await Promise.allSettled([Promise.resolve(10), Promise.resolve(20)]); print(results.length, results[0].value, results[1].value);"
            ),
            "2 10 20\n"
        );
    }

    // --- Async function tests ---

    #[test]
    fn test_async_function_basic() {
        assert_eq!(
            eval_promise(
                "async function foo() { return 42; } let p = foo(); let val = await p; print(val);"
            ),
            "42\n"
        );
    }

    #[test]
    fn test_async_function_with_await() {
        assert_eq!(
            eval_promise(
                "async function fetch() { return await Promise.resolve(\"data\"); } let result = await fetch(); print(result);"
            ),
            "data\n"
        );
    }

    #[test]
    fn test_async_arrow_basic() {
        assert_eq!(
            eval_promise(
                "let f = async () => 42; let val = await f(); print(val);"
            ),
            "42\n"
        );
    }
}
