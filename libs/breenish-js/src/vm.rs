//! Stack-based virtual machine for the breenish-js engine.
//!
//! Executes bytecode produced by the compiler using a value stack
//! and call frame stack.

use alloc::string::String;
use alloc::vec::Vec;

use crate::bytecode::{CodeBlock, Constant, Op};
use crate::error::{JsError, JsResult};
use crate::string::{StringId, StringPool};
use crate::value::JsValue;

/// Maximum stack depth to prevent infinite recursion.
const MAX_STACK_SIZE: usize = 4096;
/// Maximum call depth.
const MAX_CALL_DEPTH: usize = 256;

/// A call frame representing a function invocation.
#[derive(Debug)]
struct CallFrame {
    /// Index into the function table (or usize::MAX for the main script).
    func_index: usize,
    /// Instruction pointer (offset into the code block).
    ip: usize,
    /// Base index in the value stack for this frame's locals.
    base: usize,
}

/// A global variable.
struct Global {
    name: StringId,
    value: JsValue,
}

/// Host callback type for print output.
pub type PrintFn = fn(&str);

/// The JavaScript virtual machine.
pub struct Vm {
    /// The value stack.
    stack: Vec<JsValue>,
    /// Call frame stack.
    frames: Vec<CallFrame>,
    /// Global variables.
    globals: Vec<Global>,
    /// Host print callback.
    print_fn: PrintFn,
}

fn default_print(s: &str) {
    #[cfg(feature = "std")]
    {
        extern crate std;
        std::print!("{}", s);
    }
    #[cfg(not(feature = "std"))]
    {
        let _ = s;
    }
}

impl Vm {
    pub fn new() -> Self {
        Self {
            stack: Vec::with_capacity(256),
            frames: Vec::with_capacity(16),
            globals: Vec::new(),
            print_fn: default_print,
        }
    }

    /// Set the host print callback.
    pub fn set_print_fn(&mut self, f: PrintFn) {
        self.print_fn = f;
    }

    /// Execute a compiled program.
    pub fn execute(
        &mut self,
        code: &CodeBlock,
        strings: &mut StringPool,
        functions: &[CodeBlock],
    ) -> JsResult<JsValue> {
        // Initialize locals for main script
        let local_count = code.local_count as usize;
        for _ in 0..local_count {
            self.stack.push(JsValue::undefined());
        }

        self.frames.push(CallFrame {
            func_index: usize::MAX,
            ip: 0,
            base: 0,
        });

        self.run(code, strings, functions)
    }

    fn get_code<'a>(
        &self,
        frame_idx: usize,
        main: &'a CodeBlock,
        functions: &'a [CodeBlock],
    ) -> &'a CodeBlock {
        let fi = self.frames[frame_idx].func_index;
        if fi == usize::MAX {
            main
        } else {
            &functions[fi]
        }
    }

    /// Read and advance the IP by 1, returning the byte read.
    fn read_byte(&mut self, code: &CodeBlock) -> u8 {
        let fi = self.frames.len() - 1;
        let ip = self.frames[fi].ip;
        self.frames[fi].ip += 1;
        code.code[ip]
    }

    /// Read a u16 at current IP and advance IP by 2.
    fn read_u16_advance(&mut self, code: &CodeBlock) -> u16 {
        let fi = self.frames.len() - 1;
        let ip = self.frames[fi].ip;
        self.frames[fi].ip += 2;
        code.read_u16(ip)
    }

    /// Read a u8 at current IP and advance IP by 1.
    fn read_u8_advance(&mut self, code: &CodeBlock) -> u8 {
        let fi = self.frames.len() - 1;
        let ip = self.frames[fi].ip;
        self.frames[fi].ip += 1;
        code.code[ip]
    }

    fn current_ip(&self) -> usize {
        self.frames.last().unwrap().ip
    }

    fn set_ip(&mut self, ip: usize) {
        self.frames.last_mut().unwrap().ip = ip;
    }

    fn current_base(&self) -> usize {
        self.frames.last().unwrap().base
    }

    fn run(
        &mut self,
        main: &CodeBlock,
        strings: &mut StringPool,
        functions: &[CodeBlock],
    ) -> JsResult<JsValue> {
        loop {
            let fi = self.frames.len() - 1;
            let code = self.get_code(fi, main, functions);

            if self.frames[fi].ip >= code.code.len() {
                if self.frames.len() == 1 {
                    return Ok(if self.stack.is_empty() {
                        JsValue::undefined()
                    } else {
                        *self.stack.last().unwrap()
                    });
                }
                let result = self.stack.pop().unwrap_or(JsValue::undefined());
                let old_base = self.frames[fi].base;
                self.frames.pop();
                self.stack.truncate(old_base);
                self.stack.push(result);
                continue;
            }

            let op_byte = self.read_byte(code);

            let Some(op) = Op::from_byte(op_byte) else {
                return Err(JsError::internal(alloc::format!(
                    "unknown opcode 0x{:02x}",
                    op_byte
                )));
            };

            match op {
                Op::LoadConst => {
                    let idx = self.read_u16_advance(code) as usize;
                    let value = match &code.constants[idx] {
                        Constant::Number(n) => JsValue::number(*n),
                        Constant::String(sid) => JsValue::string(StringId(*sid)),
                        Constant::Function(fi) => JsValue::object(*fi | 0x80000000),
                    };
                    self.push(value)?;
                }

                Op::LoadLocal => {
                    let slot = self.read_u16_advance(code) as usize;
                    let base = self.current_base();
                    let value = self.stack[base + slot];
                    self.push(value)?;
                }

                Op::StoreLocal => {
                    let slot = self.read_u16_advance(code) as usize;
                    let value = self.pop();
                    let base = self.current_base();
                    self.stack[base + slot] = value;
                }

                Op::LoadGlobal => {
                    let idx = self.read_u16_advance(code) as usize;
                    let name_sid = match &code.constants[idx] {
                        Constant::String(sid) => StringId(*sid),
                        _ => {
                            return Err(JsError::internal("LoadGlobal: expected string constant"));
                        }
                    };
                    let value = self.get_global(name_sid);
                    self.push(value)?;
                }

                Op::StoreGlobal => {
                    let idx = self.read_u16_advance(code) as usize;
                    let value = self.pop();
                    let name_sid = match &code.constants[idx] {
                        Constant::String(sid) => StringId(*sid),
                        _ => {
                            return Err(JsError::internal(
                                "StoreGlobal: expected string constant",
                            ));
                        }
                    };
                    self.set_global(name_sid, value);
                }

                Op::Add => {
                    let b = self.pop();
                    let a = self.pop();
                    if a.is_string() || b.is_string() {
                        let sa = self.value_to_string(a, strings);
                        let sb = self.value_to_string(b, strings);
                        let result = alloc::format!("{}{}", sa, sb);
                        let id = strings.intern(&result);
                        self.push(JsValue::string(id))?;
                    } else {
                        self.push(JsValue::number(a.to_number() + b.to_number()))?;
                    }
                }

                Op::Sub => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(a.to_number() - b.to_number()))?;
                }

                Op::Mul => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(a.to_number() * b.to_number()))?;
                }

                Op::Div => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(a.to_number() / b.to_number()))?;
                }

                Op::Mod => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(a.to_number() % b.to_number()))?;
                }

                Op::Negate => {
                    let a = self.pop();
                    self.push(JsValue::number(-a.to_number()))?;
                }

                Op::Equal | Op::StrictEqual => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(if a.strict_equal(&b) {
                        1.0
                    } else {
                        0.0
                    }))?;
                }

                Op::NotEqual | Op::StrictNotEqual => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(if !a.strict_equal(&b) {
                        1.0
                    } else {
                        0.0
                    }))?;
                }

                Op::LessThan => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(if a.to_number() < b.to_number() {
                        1.0
                    } else {
                        0.0
                    }))?;
                }

                Op::GreaterThan => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(if a.to_number() > b.to_number() {
                        1.0
                    } else {
                        0.0
                    }))?;
                }

                Op::LessEqual => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(if a.to_number() <= b.to_number() {
                        1.0
                    } else {
                        0.0
                    }))?;
                }

                Op::GreaterEqual => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(JsValue::number(if a.to_number() >= b.to_number() {
                        1.0
                    } else {
                        0.0
                    }))?;
                }

                Op::Not => {
                    let a = self.pop();
                    self.push(JsValue::number(if a.to_boolean() { 0.0 } else { 1.0 }))?;
                }

                Op::TypeOf => {
                    let a = self.pop();
                    let type_name = a.type_name();
                    let id = strings.intern(type_name);
                    self.push(JsValue::string(id))?;
                }

                Op::Jump => {
                    let target = code.read_u16(self.current_ip()) as usize;
                    self.set_ip(target);
                }

                Op::JumpIfFalse => {
                    let target = self.read_u16_advance(code) as usize;
                    let val = self.pop();
                    if !val.to_boolean() {
                        self.set_ip(target);
                    }
                }

                Op::JumpIfTrue => {
                    let target = self.read_u16_advance(code) as usize;
                    let val = self.pop();
                    if val.to_boolean() {
                        self.set_ip(target);
                    }
                }

                Op::Call => {
                    let argc = self.read_u8_advance(code) as usize;

                    let func_pos = self.stack.len() - argc - 1;
                    let func_val = self.stack[func_pos];

                    if !func_val.is_object() {
                        return Err(JsError::type_error("not a function"));
                    }

                    let obj_idx = func_val.as_object_index();
                    if obj_idx & 0x80000000 == 0 {
                        return Err(JsError::type_error("not a function"));
                    }

                    let func_index = (obj_idx & 0x7FFFFFFF) as usize;
                    if func_index >= functions.len() {
                        return Err(JsError::internal("invalid function index"));
                    }

                    if self.frames.len() >= MAX_CALL_DEPTH {
                        return Err(JsError::range_error("maximum call stack size exceeded"));
                    }

                    let func = &functions[func_index];
                    let needed_locals = func.local_count as usize;

                    // Remove the function value from the stack, shifting args down
                    self.stack.remove(func_pos);
                    let base = func_pos;

                    // Pad with undefined if fewer args than params
                    while self.stack.len() - base < needed_locals {
                        self.stack.push(JsValue::undefined());
                    }

                    self.frames.push(CallFrame {
                        func_index,
                        ip: 0,
                        base,
                    });
                }

                Op::Return => {
                    let result = self.pop();

                    if self.frames.len() <= 1 {
                        return Ok(result);
                    }

                    let old_frame = self.frames.pop().unwrap();
                    self.stack.truncate(old_frame.base);
                    self.push(result)?;
                }

                Op::Pop => {
                    self.pop();
                }

                Op::Dup => {
                    let val = *self.stack.last().unwrap_or(&JsValue::undefined());
                    self.push(val)?;
                }

                Op::Print => {
                    let argc = self.read_u8_advance(code) as usize;

                    let mut parts: Vec<String> = Vec::new();
                    let start = self.stack.len() - argc;
                    for i in start..self.stack.len() {
                        parts.push(self.value_to_string(self.stack[i], strings));
                    }
                    for _ in 0..argc {
                        self.pop();
                    }

                    let output = parts.join(" ");
                    (self.print_fn)(&output);
                    (self.print_fn)("\n");

                    self.push(JsValue::undefined())?;
                }

                Op::Concat => {
                    let b = self.pop();
                    let a = self.pop();
                    let sa = self.value_to_string(a, strings);
                    let sb = self.value_to_string(b, strings);
                    let result = alloc::format!("{}{}", sa, sb);
                    let id = strings.intern(&result);
                    self.push(JsValue::string(id))?;
                }

                Op::Halt => {
                    return Ok(if self.stack.is_empty() {
                        JsValue::undefined()
                    } else {
                        self.pop()
                    });
                }
            }
        }
    }

    fn push(&mut self, value: JsValue) -> JsResult<()> {
        if self.stack.len() >= MAX_STACK_SIZE {
            return Err(JsError::range_error("stack overflow"));
        }
        self.stack.push(value);
        Ok(())
    }

    fn pop(&mut self) -> JsValue {
        self.stack.pop().unwrap_or(JsValue::undefined())
    }

    fn get_global(&self, name: StringId) -> JsValue {
        for g in &self.globals {
            if g.name == name {
                return g.value;
            }
        }
        JsValue::undefined()
    }

    fn set_global(&mut self, name: StringId, value: JsValue) {
        for g in &mut self.globals {
            if g.name == name {
                g.value = value;
                return;
            }
        }
        self.globals.push(Global { name, value });
    }

    fn value_to_string(&self, value: JsValue, strings: &StringPool) -> String {
        if value.is_string() {
            String::from(strings.get(value.as_string_id()))
        } else {
            value.to_number_string()
        }
    }
}
