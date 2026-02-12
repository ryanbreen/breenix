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

use alloc::string::String as AllocString;
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

    /// Register built-in JSON support.
    ///
    /// This registers JSON.parse and JSON.stringify as native functions
    /// and creates a JSON global object.
    pub fn register_json_builtins(&mut self) {
        use crate::object::{JsObject, ObjectHeap};
        use crate::string::StringPool as SP;
        use crate::value::JsValue;
        use crate::error::{JsError, JsResult};

        // --- JSON parser (recursive descent on &[u8]) ---

        struct JsonParser<'a> {
            data: &'a [u8],
            pos: usize,
        }

        impl<'a> JsonParser<'a> {
            fn new(data: &'a [u8]) -> Self {
                Self { data, pos: 0 }
            }

            fn skip_whitespace(&mut self) {
                while self.pos < self.data.len() {
                    match self.data[self.pos] {
                        b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                        _ => break,
                    }
                }
            }

            fn peek(&mut self) -> Option<u8> {
                self.skip_whitespace();
                self.data.get(self.pos).copied()
            }

            fn advance(&mut self) -> Option<u8> {
                if self.pos < self.data.len() {
                    let c = self.data[self.pos];
                    self.pos += 1;
                    Some(c)
                } else {
                    None
                }
            }

            fn expect(&mut self, ch: u8) -> JsResult<()> {
                self.skip_whitespace();
                match self.advance() {
                    Some(c) if c == ch => Ok(()),
                    Some(c) => Err(JsError::syntax(
                        alloc::format!("expected '{}', got '{}'", ch as char, c as char),
                        0, self.pos as u32,
                    )),
                    None => Err(JsError::syntax(
                        alloc::format!("expected '{}', got EOF", ch as char),
                        0, self.pos as u32,
                    )),
                }
            }

            fn parse_value(&mut self, strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
                match self.peek() {
                    Some(b'"') => self.parse_string_value(strings),
                    Some(b'{') => self.parse_object(strings, heap),
                    Some(b'[') => self.parse_array(strings, heap),
                    Some(b't') => self.parse_true(),
                    Some(b'f') => self.parse_false(),
                    Some(b'n') => self.parse_null(),
                    Some(c) if c == b'-' || c.is_ascii_digit() => self.parse_number(),
                    Some(c) => Err(JsError::syntax(
                        alloc::format!("unexpected character '{}' in JSON", c as char),
                        0, self.pos as u32,
                    )),
                    None => Err(JsError::syntax("unexpected end of JSON input", 0, 0)),
                }
            }

            fn parse_string_raw(&mut self) -> JsResult<AllocString> {
                self.skip_whitespace();
                self.expect(b'"')?;
                let mut s = AllocString::new();
                loop {
                    match self.advance() {
                        Some(b'"') => return Ok(s),
                        Some(b'\\') => {
                            match self.advance() {
                                Some(b'"') => s.push('"'),
                                Some(b'\\') => s.push('\\'),
                                Some(b'/') => s.push('/'),
                                Some(b'n') => s.push('\n'),
                                Some(b't') => s.push('\t'),
                                Some(b'r') => s.push('\r'),
                                Some(b'b') => s.push('\u{0008}'),
                                Some(b'f') => s.push('\u{000C}'),
                                Some(b'u') => {
                                    let mut hex = AllocString::new();
                                    for _ in 0..4 {
                                        match self.advance() {
                                            Some(c) if c.is_ascii_hexdigit() => hex.push(c as char),
                                            _ => return Err(JsError::syntax(
                                                "invalid unicode escape in JSON string",
                                                0, self.pos as u32,
                                            )),
                                        }
                                    }
                                    let code = u32::from_str_radix(&hex, 16).map_err(|_| {
                                        JsError::syntax("invalid unicode escape", 0, self.pos as u32)
                                    })?;
                                    if let Some(ch) = char::from_u32(code) {
                                        s.push(ch);
                                    } else {
                                        return Err(JsError::syntax(
                                            "invalid unicode code point",
                                            0, self.pos as u32,
                                        ));
                                    }
                                }
                                Some(c) => {
                                    return Err(JsError::syntax(
                                        alloc::format!("invalid escape '\\{}'", c as char),
                                        0, self.pos as u32,
                                    ));
                                }
                                None => return Err(JsError::syntax(
                                    "unterminated string escape",
                                    0, self.pos as u32,
                                )),
                            }
                        }
                        Some(c) => s.push(c as char),
                        None => return Err(JsError::syntax(
                            "unterminated JSON string",
                            0, self.pos as u32,
                        )),
                    }
                }
            }

            fn parse_string_value(&mut self, strings: &mut SP) -> JsResult<JsValue> {
                let s = self.parse_string_raw()?;
                let id = strings.intern(&s);
                Ok(JsValue::string(id))
            }

            fn parse_number(&mut self) -> JsResult<JsValue> {
                self.skip_whitespace();
                let start = self.pos;
                // optional leading minus
                if self.pos < self.data.len() && self.data[self.pos] == b'-' {
                    self.pos += 1;
                }
                // integer part
                while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
                // fractional part
                if self.pos < self.data.len() && self.data[self.pos] == b'.' {
                    self.pos += 1;
                    while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                        self.pos += 1;
                    }
                }
                // exponent
                if self.pos < self.data.len() && (self.data[self.pos] == b'e' || self.data[self.pos] == b'E') {
                    self.pos += 1;
                    if self.pos < self.data.len() && (self.data[self.pos] == b'+' || self.data[self.pos] == b'-') {
                        self.pos += 1;
                    }
                    while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                        self.pos += 1;
                    }
                }
                let num_str = core::str::from_utf8(&self.data[start..self.pos])
                    .map_err(|_| JsError::syntax("invalid number", 0, start as u32))?;
                let val: f64 = num_str.parse()
                    .map_err(|_| JsError::syntax(
                        alloc::format!("invalid number: {}", num_str),
                        0, start as u32,
                    ))?;
                Ok(JsValue::number(val))
            }

            fn parse_true(&mut self) -> JsResult<JsValue> {
                self.skip_whitespace();
                if self.data[self.pos..].starts_with(b"true") {
                    self.pos += 4;
                    Ok(JsValue::boolean(true))
                } else {
                    Err(JsError::syntax("invalid JSON value", 0, self.pos as u32))
                }
            }

            fn parse_false(&mut self) -> JsResult<JsValue> {
                self.skip_whitespace();
                if self.data[self.pos..].starts_with(b"false") {
                    self.pos += 5;
                    Ok(JsValue::boolean(false))
                } else {
                    Err(JsError::syntax("invalid JSON value", 0, self.pos as u32))
                }
            }

            fn parse_null(&mut self) -> JsResult<JsValue> {
                self.skip_whitespace();
                if self.data[self.pos..].starts_with(b"null") {
                    self.pos += 4;
                    Ok(JsValue::null())
                } else {
                    Err(JsError::syntax("invalid JSON value", 0, self.pos as u32))
                }
            }

            fn parse_object(&mut self, strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
                self.skip_whitespace();
                self.expect(b'{')?;
                let mut obj = JsObject::new();

                if self.peek() == Some(b'}') {
                    self.advance();
                    let idx = heap.alloc(obj);
                    return Ok(JsValue::object(idx));
                }

                loop {
                    let key = self.parse_string_raw()?;
                    self.expect(b':')?;
                    let value = self.parse_value(strings, heap)?;
                    let key_id = strings.intern(&key);
                    obj.set(key_id, value);

                    match self.peek() {
                        Some(b',') => { self.advance(); }
                        Some(b'}') => { self.advance(); break; }
                        _ => return Err(JsError::syntax(
                            "expected ',' or '}' in JSON object",
                            0, self.pos as u32,
                        )),
                    }
                }

                let idx = heap.alloc(obj);
                Ok(JsValue::object(idx))
            }

            fn parse_array(&mut self, strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
                self.skip_whitespace();
                self.expect(b'[')?;
                let mut arr = JsObject::new_array();

                if self.peek() == Some(b']') {
                    self.advance();
                    let idx = heap.alloc(arr);
                    return Ok(JsValue::object(idx));
                }

                loop {
                    let value = self.parse_value(strings, heap)?;
                    arr.push(value);

                    match self.peek() {
                        Some(b',') => { self.advance(); }
                        Some(b']') => { self.advance(); break; }
                        _ => return Err(JsError::syntax(
                            "expected ',' or ']' in JSON array",
                            0, self.pos as u32,
                        )),
                    }
                }

                let idx = heap.alloc(arr);
                Ok(JsValue::object(idx))
            }
        }

        // --- JSON stringify ---

        fn json_stringify_value(
            value: JsValue,
            strings: &SP,
            heap: &ObjectHeap,
            visited: &mut Vec<u32>,
        ) -> JsResult<AllocString> {
            if value.is_null() {
                Ok(AllocString::from("null"))
            } else if value.is_undefined() {
                // undefined is not valid JSON, but we output it for consistency
                Ok(AllocString::from("undefined"))
            } else if value.is_boolean() {
                if value.as_boolean() {
                    Ok(AllocString::from("true"))
                } else {
                    Ok(AllocString::from("false"))
                }
            } else if value.is_number() {
                let n = value.to_number();
                if n.is_nan() {
                    Ok(AllocString::from("null"))
                } else if n.is_infinite() {
                    Ok(AllocString::from("null"))
                } else if n == n.floor() && n.abs() < 1e15 {
                    Ok(alloc::format!("{}", n as i64))
                } else {
                    Ok(alloc::format!("{}", n))
                }
            } else if value.is_string() {
                let s = strings.get(value.as_string_id());
                Ok(json_escape_string(s))
            } else if value.is_object() {
                let obj_idx = value.as_object_index();
                // Circular reference detection
                if visited.contains(&obj_idx) {
                    return Err(JsError::type_error("circular reference in JSON.stringify"));
                }
                visited.push(obj_idx);

                let result = if let Some(obj) = heap.get(obj_idx) {
                    if obj.kind == crate::object::ObjectKind::Array {
                        stringify_array(obj_idx, strings, heap, visited)
                    } else {
                        stringify_object(obj_idx, strings, heap, visited)
                    }
                } else {
                    Ok(AllocString::from("null"))
                };

                visited.pop();
                result
            } else {
                Ok(AllocString::from("null"))
            }
        }

        fn json_escape_string(s: &str) -> AllocString {
            let mut out = AllocString::from("\"");
            for ch in s.chars() {
                match ch {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\u{0008}' => out.push_str("\\b"),
                    '\u{000C}' => out.push_str("\\f"),
                    c if (c as u32) < 0x20 => {
                        out.push_str(&alloc::format!("\\u{:04x}", c as u32));
                    }
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }

        fn stringify_array(
            obj_idx: u32,
            strings: &SP,
            heap: &ObjectHeap,
            visited: &mut Vec<u32>,
        ) -> JsResult<AllocString> {
            let elements: Vec<JsValue> = heap.get(obj_idx)
                .map(|obj| obj.elements().to_vec())
                .unwrap_or_default();

            let mut out = AllocString::from("[");
            for (i, elem) in elements.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let s = json_stringify_value(*elem, strings, heap, visited)?;
                out.push_str(&s);
            }
            out.push(']');
            Ok(out)
        }

        fn stringify_object(
            obj_idx: u32,
            strings: &SP,
            heap: &ObjectHeap,
            visited: &mut Vec<u32>,
        ) -> JsResult<AllocString> {
            let (keys, values): (Vec<crate::string::StringId>, Vec<JsValue>) = heap.get(obj_idx)
                .map(|obj| {
                    let ks = obj.keys();
                    let vs: Vec<JsValue> = ks.iter().map(|k| obj.get(*k)).collect();
                    (ks, vs)
                })
                .unwrap_or_default();

            let mut out = AllocString::from("{");
            let mut first = true;
            for (key, value) in keys.iter().zip(values.iter()) {
                // Skip undefined values in objects per JSON spec
                if value.is_undefined() {
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                let key_str = strings.get(*key);
                out.push_str(&json_escape_string(key_str));
                out.push(':');
                let val_str = json_stringify_value(*value, strings, heap, visited)?;
                out.push_str(&val_str);
            }
            out.push('}');
            Ok(out)
        }

        // --- Native function implementations ---

        fn json_parse(args: &[JsValue], strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let input = args.first().copied().unwrap_or(JsValue::undefined());
            if !input.is_string() {
                return Err(JsError::syntax("JSON.parse: expected string argument", 0, 0));
            }
            let s = AllocString::from(strings.get(input.as_string_id()));
            let mut parser = JsonParser::new(s.as_bytes());
            let result = parser.parse_value(strings, heap)?;
            // Ensure no trailing content
            parser.skip_whitespace();
            if parser.pos < parser.data.len() {
                return Err(JsError::syntax("unexpected content after JSON value", 0, parser.pos as u32));
            }
            Ok(result)
        }

        fn json_stringify(args: &[JsValue], strings: &mut SP, heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let value = args.first().copied().unwrap_or(JsValue::undefined());
            let mut visited = Vec::new();
            let result = json_stringify_value(value, strings, heap, &mut visited)?;
            let id = strings.intern(&result);
            Ok(JsValue::string(id))
        }

        // Register the native functions
        self.vm.register_native("JSON_parse", json_parse);
        self.vm.register_native("JSON_stringify", json_stringify);

        // Build the JSON global object
        let parse_idx = self.vm.native_obj_indices[self.vm.native_obj_indices.len() - 2];
        let stringify_idx = self.vm.native_obj_indices[self.vm.native_obj_indices.len() - 1];

        let mut json_obj = JsObject::new();
        let parse_key = self.strings.intern("parse");
        let stringify_key = self.strings.intern("stringify");
        json_obj.set(parse_key, JsValue::object(parse_idx));
        json_obj.set(stringify_key, JsValue::object(stringify_idx));

        let obj_idx = self.vm.heap.alloc(json_obj);
        self.vm.set_global_by_name("JSON", JsValue::object(obj_idx), &mut self.strings);
    }

    /// Register built-in Math and Number objects.
    ///
    /// This registers Math.floor, Math.ceil, Math.round, Math.abs, Math.min,
    /// Math.max, Math.pow, Math.sqrt, Math.random, Math.log, Math.trunc,
    /// Math.PI, Math.E, Number.isInteger, Number.isFinite, Number.isNaN,
    /// Number.parseInt, Number.parseFloat, and global parseInt/parseFloat.
    pub fn register_math_builtins(&mut self) {
        use crate::object::{JsObject, ObjectHeap};
        use crate::string::StringPool as SP;
        use crate::value::JsValue;
        use crate::error::JsResult;

        fn math_floor(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let x = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(x.floor()))
        }

        fn math_ceil(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let x = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(x.ceil()))
        }

        fn math_round(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let x = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(x.round()))
        }

        fn math_abs(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let x = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(x.abs()))
        }

        fn math_min(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let a = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            let b = args.get(1).copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(a.min(b)))
        }

        fn math_max(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let a = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            let b = args.get(1).copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(a.max(b)))
        }

        fn math_pow(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let base = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            let exp = args.get(1).copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(base.powf(exp)))
        }

        fn math_sqrt(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let x = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(x.sqrt()))
        }

        fn math_random(_args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            // Simple xorshift64 PRNG (single-threaded, suitable for no_std)
            static mut RANDOM_STATE: u64 = 0x12345678;
            unsafe {
                RANDOM_STATE ^= RANDOM_STATE << 13;
                RANDOM_STATE ^= RANDOM_STATE >> 7;
                RANDOM_STATE ^= RANDOM_STATE << 17;
                let val = (RANDOM_STATE as f64) / (u64::MAX as f64);
                Ok(JsValue::number(val))
            }
        }

        fn math_log(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let x = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(x.ln()))
        }

        fn math_trunc(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let x = args.first().copied().unwrap_or(JsValue::undefined()).to_number();
            Ok(JsValue::number(x.trunc()))
        }

        fn number_is_integer(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            if !val.is_number() {
                return Ok(JsValue::number(0.0));
            }
            let x = val.to_number();
            let result = x.is_finite() && x == x.floor();
            Ok(JsValue::number(if result { 1.0 } else { 0.0 }))
        }

        fn number_is_finite(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            if !val.is_number() {
                return Ok(JsValue::number(0.0));
            }
            let x = val.to_number();
            Ok(JsValue::number(if x.is_finite() { 1.0 } else { 0.0 }))
        }

        fn number_is_nan(args: &[JsValue], _strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            if !val.is_number() {
                return Ok(JsValue::number(0.0));
            }
            let x = val.to_number();
            Ok(JsValue::number(if x.is_nan() { 1.0 } else { 0.0 }))
        }

        fn parse_int(args: &[JsValue], strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            if val.is_number() {
                let x = val.to_number();
                return Ok(JsValue::number(x.trunc()));
            }
            if val.is_string() {
                let s = strings.get(val.as_string_id());
                let trimmed = s.trim();
                if let Ok(n) = trimmed.parse::<i64>() {
                    return Ok(JsValue::number(n as f64));
                }
                // Try parsing as float and truncating
                if let Ok(n) = trimmed.parse::<f64>() {
                    return Ok(JsValue::number(n.trunc()));
                }
            }
            Ok(JsValue::number(f64::NAN))
        }

        fn parse_float(args: &[JsValue], strings: &mut SP, _heap: &mut ObjectHeap) -> JsResult<JsValue> {
            let val = args.first().copied().unwrap_or(JsValue::undefined());
            if val.is_number() {
                return Ok(val);
            }
            if val.is_string() {
                let s = strings.get(val.as_string_id());
                let trimmed = s.trim();
                if let Ok(n) = trimmed.parse::<f64>() {
                    return Ok(JsValue::number(n));
                }
            }
            Ok(JsValue::number(f64::NAN))
        }

        // Register all native functions
        self.vm.register_native("Math_floor", math_floor);
        self.vm.register_native("Math_ceil", math_ceil);
        self.vm.register_native("Math_round", math_round);
        self.vm.register_native("Math_abs", math_abs);
        self.vm.register_native("Math_min", math_min);
        self.vm.register_native("Math_max", math_max);
        self.vm.register_native("Math_pow", math_pow);
        self.vm.register_native("Math_sqrt", math_sqrt);
        self.vm.register_native("Math_random", math_random);
        self.vm.register_native("Math_log", math_log);
        self.vm.register_native("Math_trunc", math_trunc);
        self.vm.register_native("Number_isInteger", number_is_integer);
        self.vm.register_native("Number_isFinite", number_is_finite);
        self.vm.register_native("Number_isNaN", number_is_nan);
        self.vm.register_native("parseInt", parse_int);
        self.vm.register_native("parseFloat", parse_float);

        // Build the Math global object
        let num_natives = 16;
        let base = self.vm.native_obj_indices.len() - num_natives;
        let floor_obj_idx = self.vm.native_obj_indices[base];
        let ceil_obj_idx = self.vm.native_obj_indices[base + 1];
        let round_obj_idx = self.vm.native_obj_indices[base + 2];
        let abs_obj_idx = self.vm.native_obj_indices[base + 3];
        let min_obj_idx = self.vm.native_obj_indices[base + 4];
        let max_obj_idx = self.vm.native_obj_indices[base + 5];
        let pow_obj_idx = self.vm.native_obj_indices[base + 6];
        let sqrt_obj_idx = self.vm.native_obj_indices[base + 7];
        let random_obj_idx = self.vm.native_obj_indices[base + 8];
        let log_obj_idx = self.vm.native_obj_indices[base + 9];
        let trunc_obj_idx = self.vm.native_obj_indices[base + 10];
        let is_integer_obj_idx = self.vm.native_obj_indices[base + 11];
        let is_finite_obj_idx = self.vm.native_obj_indices[base + 12];
        let is_nan_obj_idx = self.vm.native_obj_indices[base + 13];

        let mut math_obj = JsObject::new();
        let floor_key = self.strings.intern("floor");
        let ceil_key = self.strings.intern("ceil");
        let round_key = self.strings.intern("round");
        let abs_key = self.strings.intern("abs");
        let min_key = self.strings.intern("min");
        let max_key = self.strings.intern("max");
        let pow_key = self.strings.intern("pow");
        let sqrt_key = self.strings.intern("sqrt");
        let random_key = self.strings.intern("random");
        let log_key = self.strings.intern("log");
        let trunc_key = self.strings.intern("trunc");
        let pi_key = self.strings.intern("PI");
        let e_key = self.strings.intern("E");

        math_obj.set(floor_key, JsValue::object(floor_obj_idx));
        math_obj.set(ceil_key, JsValue::object(ceil_obj_idx));
        math_obj.set(round_key, JsValue::object(round_obj_idx));
        math_obj.set(abs_key, JsValue::object(abs_obj_idx));
        math_obj.set(min_key, JsValue::object(min_obj_idx));
        math_obj.set(max_key, JsValue::object(max_obj_idx));
        math_obj.set(pow_key, JsValue::object(pow_obj_idx));
        math_obj.set(sqrt_key, JsValue::object(sqrt_obj_idx));
        math_obj.set(random_key, JsValue::object(random_obj_idx));
        math_obj.set(log_key, JsValue::object(log_obj_idx));
        math_obj.set(trunc_key, JsValue::object(trunc_obj_idx));
        math_obj.set(pi_key, JsValue::number(core::f64::consts::PI));
        math_obj.set(e_key, JsValue::number(core::f64::consts::E));

        let math_idx = self.vm.heap.alloc(math_obj);
        self.vm.set_global_by_name("Math", JsValue::object(math_idx), &mut self.strings);

        // Build the Number global object
        let mut number_obj = JsObject::new();
        let is_integer_key = self.strings.intern("isInteger");
        let is_finite_key = self.strings.intern("isFinite");
        let is_nan_key = self.strings.intern("isNaN");
        let parse_int_key = self.strings.intern("parseInt");
        let parse_float_key = self.strings.intern("parseFloat");

        number_obj.set(is_integer_key, JsValue::object(is_integer_obj_idx));
        number_obj.set(is_finite_key, JsValue::object(is_finite_obj_idx));
        number_obj.set(is_nan_key, JsValue::object(is_nan_obj_idx));
        let parse_int_obj_idx = self.vm.native_obj_indices[base + 14];
        let parse_float_obj_idx = self.vm.native_obj_indices[base + 15];
        number_obj.set(parse_int_key, JsValue::object(parse_int_obj_idx));
        number_obj.set(parse_float_key, JsValue::object(parse_float_obj_idx));

        let number_idx = self.vm.heap.alloc(number_obj);
        self.vm.set_global_by_name("Number", JsValue::object(number_idx), &mut self.strings);
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

    // --- JSON tests ---

    fn eval_json(source: &str) -> String {
        let mut ctx = Context::new();
        ctx.set_print_fn(capture_print);
        ctx.register_promise_builtins();
        ctx.register_json_builtins();
        ctx.eval(source).unwrap();
        take_output()
    }

    #[test]
    fn test_json_parse_number() {
        assert_eq!(eval_json("print(JSON.parse(\"42\"));"), "42\n");
    }

    #[test]
    fn test_json_parse_string() {
        assert_eq!(
            eval_json("print(JSON.parse('\"hello\"'));"),
            "hello\n"
        );
    }

    #[test]
    fn test_json_parse_object() {
        assert_eq!(
            eval_json("let o = JSON.parse('{\"a\":1,\"b\":\"two\"}'); print(o.a, o.b);"),
            "1 two\n"
        );
    }

    #[test]
    fn test_json_parse_array() {
        assert_eq!(
            eval_json("let a = JSON.parse('[1,2,3]'); print(a[0], a[1], a[2]);"),
            "1 2 3\n"
        );
    }

    #[test]
    fn test_json_parse_nested() {
        assert_eq!(
            eval_json("let o = JSON.parse('{\"arr\":[1,2],\"obj\":{\"x\":true}}'); print(o.arr[0], o.arr[1], o.obj.x);"),
            "1 2 true\n"
        );
    }

    #[test]
    fn test_json_parse_null() {
        assert_eq!(eval_json("print(JSON.parse(\"null\"));"), "null\n");
    }

    #[test]
    fn test_json_stringify_number() {
        assert_eq!(eval_json("print(JSON.stringify(42));"), "42\n");
    }

    #[test]
    fn test_json_stringify_string() {
        assert_eq!(
            eval_json("print(JSON.stringify(\"hello\"));"),
            "\"hello\"\n"
        );
    }

    #[test]
    fn test_json_stringify_object() {
        assert_eq!(
            eval_json("print(JSON.stringify({a: 1, b: \"two\"}));"),
            "{\"a\":1,\"b\":\"two\"}\n"
        );
    }

    #[test]
    fn test_json_stringify_array() {
        assert_eq!(
            eval_json("print(JSON.stringify([1, 2, 3]));"),
            "[1,2,3]\n"
        );
    }

    #[test]
    fn test_json_stringify_nested() {
        // Note: the JS compiler represents `true` as JsValue::number(1.0),
        // so JSON.stringify outputs 1 instead of true for JS-created booleans.
        // JSON.parse("true") creates a proper JsValue::boolean which roundtrips correctly.
        assert_eq!(
            eval_json("let o = {arr: [1, 2], obj: {x: true}}; print(JSON.stringify(o));"),
            "{\"arr\":[1,2],\"obj\":{\"x\":1}}\n"
        );
    }

    #[test]
    fn test_json_roundtrip() {
        assert_eq!(
            eval_json("let o = {x: [1,2,3]}; let s = JSON.stringify(o); let o2 = JSON.parse(s); print(o2.x[0], o2.x[1], o2.x[2]);"),
            "1 2 3\n"
        );
    }

    #[test]
    fn test_json_boolean_roundtrip() {
        // JSON.parse creates proper JsValue::boolean values,
        // which JSON.stringify correctly serializes as true/false
        assert_eq!(
            eval_json("let s = JSON.stringify(JSON.parse('{\"a\":true,\"b\":false}')); print(s);"),
            "{\"a\":true,\"b\":false}\n"
        );
    }

    // --- Math and Number tests ---

    fn eval_math(source: &str) -> String {
        let mut ctx = Context::new();
        ctx.set_print_fn(capture_print);
        ctx.register_math_builtins();
        ctx.eval(source).unwrap();
        take_output()
    }

    #[test]
    fn test_math_floor() {
        assert_eq!(eval_math("print(Math.floor(3.7));"), "3\n");
    }

    #[test]
    fn test_math_ceil() {
        assert_eq!(eval_math("print(Math.ceil(3.2));"), "4\n");
    }

    #[test]
    fn test_math_round() {
        assert_eq!(eval_math("print(Math.round(3.5));"), "4\n");
    }

    #[test]
    fn test_math_abs() {
        assert_eq!(eval_math("print(Math.abs(-5));"), "5\n");
    }

    #[test]
    fn test_math_min_max() {
        assert_eq!(eval_math("print(Math.min(3, 7));"), "3\n");
        assert_eq!(eval_math("print(Math.max(3, 7));"), "7\n");
    }

    #[test]
    fn test_math_sqrt() {
        assert_eq!(eval_math("print(Math.sqrt(16));"), "4\n");
    }

    #[test]
    fn test_math_pow() {
        assert_eq!(eval_math("print(Math.pow(2, 10));"), "1024\n");
    }

    #[test]
    fn test_math_pi() {
        assert_eq!(eval_math("print(Math.PI);"), "3.141592653589793\n");
    }

    #[test]
    fn test_math_trunc() {
        assert_eq!(eval_math("print(Math.trunc(3.7));"), "3\n");
        assert_eq!(eval_math("print(Math.trunc(-3.7));"), "-3\n");
    }

    #[test]
    fn test_number_is_integer() {
        assert_eq!(eval_math("print(Number.isInteger(5));"), "1\n");
        assert_eq!(eval_math("print(Number.isInteger(5.5));"), "0\n");
    }

    #[test]
    fn test_number_is_nan() {
        assert_eq!(eval_math("print(Number.isNaN(0/0));"), "1\n");
        assert_eq!(eval_math("print(Number.isNaN(5));"), "0\n");
    }

    #[test]
    fn test_parse_int() {
        assert_eq!(eval_math("print(parseInt(\"42\"));"), "42\n");
    }

    #[test]
    fn test_parse_float() {
        assert_eq!(eval_math("print(parseFloat(\"3.14\"));"), "3.14\n");
    }
}
