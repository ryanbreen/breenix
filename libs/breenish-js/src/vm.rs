//! Stack-based virtual machine for the breenish-js engine.
//!
//! Executes bytecode produced by the compiler using a value stack
//! and call frame stack. Manages an object heap for objects, arrays,
//! and function closures.

use alloc::string::String;
use alloc::vec::Vec;

use crate::bytecode::{CodeBlock, Constant, Op};
use crate::error::{JsError, JsResult};
use crate::object::{JsObject, ObjectHeap, ObjectKind, PromiseState};
use crate::string::{StringId, StringPool};
use crate::value::JsValue;

/// Maximum stack depth to prevent infinite recursion.
const MAX_STACK_SIZE: usize = 4096;
/// Maximum call depth.
const MAX_CALL_DEPTH: usize = 256;

/// What kind of function call to perform.
enum CallKind {
    /// JavaScript function (func_index, optional closure object).
    Js(usize, Option<u32>),
    /// Native function (native_index).
    Native(u32),
}

/// Result of executing one VM step.
enum StepResult {
    /// Continue to next step.
    Continue,
    /// Execution complete; return this value.
    Return(JsValue),
}

/// A call frame representing a function invocation.
#[derive(Debug)]
struct CallFrame {
    /// Index into the function table (or usize::MAX for the main script).
    func_index: usize,
    /// Instruction pointer (offset into the code block).
    ip: usize,
    /// Base index in the value stack for this frame's locals.
    base: usize,
    /// If this frame executes a closure, the heap index of the closure object.
    /// Upvalues are read/written directly on the heap object.
    closure_obj: Option<u32>,
}

/// A global variable.
struct Global {
    name: StringId,
    value: JsValue,
}

/// An exception handler pushed by TryStart.
#[derive(Debug)]
struct ExceptionHandler {
    /// Bytecode address of the catch block.
    catch_addr: u16,
    /// Stack depth when TryStart was executed (to restore on catch).
    stack_depth: usize,
    /// Call frame depth when TryStart was executed.
    frame_depth: usize,
}

/// Host callback type for print output.
pub type PrintFn = fn(&str);

/// A native function callable from JavaScript.
///
/// Receives the arguments as a slice and access to the string pool and object heap.
/// Returns a JsResult<JsValue>.
pub type NativeFn = fn(args: &[JsValue], strings: &mut StringPool, heap: &mut ObjectHeap) -> JsResult<JsValue>;

/// A registered native function entry.
struct NativeEntry {
    /// The function name (stored for re-interning across string pools).
    name_str: String,
    func: NativeFn,
}

/// A persistent global that gets re-interned for each eval() string pool.
struct PersistentGlobal {
    name_str: String,
    value: JsValue,
}

/// The JavaScript virtual machine.
pub struct Vm {
    /// The value stack.
    stack: Vec<JsValue>,
    /// Call frame stack.
    frames: Vec<CallFrame>,
    /// Global variables.
    globals: Vec<Global>,
    /// Object heap.
    pub heap: ObjectHeap,
    /// Host print callback.
    print_fn: PrintFn,
    /// Exception handler stack for try/catch/finally.
    exception_handlers: Vec<ExceptionHandler>,
    /// Native function table.
    native_functions: Vec<NativeEntry>,
    /// Heap object indices for each native function.
    pub native_obj_indices: Vec<u32>,
    /// Persistent globals that survive across eval() calls.
    persistent_globals: Vec<PersistentGlobal>,
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
            heap: ObjectHeap::new(),
            print_fn: default_print,
            exception_handlers: Vec::new(),
            native_functions: Vec::new(),
            native_obj_indices: Vec::new(),
            persistent_globals: Vec::new(),
        }
    }

    /// Set the host print callback.
    pub fn set_print_fn(&mut self, f: PrintFn) {
        self.print_fn = f;
    }

    /// Register a native function that can be called from JavaScript.
    ///
    /// The function will be available as a global variable with the given name.
    /// Must be called before `execute()` and the StringPool must be the same
    /// one used during compilation.
    /// Register a native function by name.
    ///
    /// The function will be available as a global variable with the given name.
    /// Call `sync_natives` with the compiler's string pool before execution.
    pub fn register_native(&mut self, name: &str, func: NativeFn) -> u32 {
        let idx = self.native_functions.len() as u32;
        // Create a NativeFunction object on the heap
        let obj = JsObject::new_native_function(idx);
        let obj_idx = self.heap.alloc(obj);
        self.native_functions.push(NativeEntry {
            name_str: String::from(name),
            func,
        });
        // Store the heap object index for later global registration
        self.native_obj_indices.push(obj_idx);
        idx
    }

    /// Set a persistent global variable by name string.
    ///
    /// Persistent globals are re-interned into each compiler's string pool
    /// before execution, so they survive across eval() calls.
    pub fn set_global_by_name(&mut self, name: &str, value: JsValue, strings: &mut StringPool) {
        let name_id = strings.intern(name);
        self.set_global(name_id, value);
        self.persistent_globals.push(PersistentGlobal {
            name_str: String::from(name),
            value,
        });
    }

    /// Re-register native function globals using the given string pool.
    ///
    /// This must be called before execution so that global lookups use the
    /// correct string IDs for the current compilation's string pool.
    pub fn sync_natives(&mut self, old_pool: &StringPool, strings: &mut StringPool) {
        for (entry, &obj_idx) in self.native_functions.iter().zip(self.native_obj_indices.iter()) {
            let name = strings.intern(&entry.name_str);
            let mut found = false;
            for g in &mut self.globals {
                if g.value == JsValue::object(obj_idx) {
                    g.name = name;
                    found = true;
                    break;
                }
            }
            if !found {
                self.globals.push(Global {
                    name,
                    value: JsValue::object(obj_idx),
                });
            }
        }

        // Re-intern persistent globals and re-key their object properties
        for pg in &self.persistent_globals {
            let name = strings.intern(&pg.name_str);
            let mut found = false;
            for g in &mut self.globals {
                if g.value == pg.value {
                    g.name = name;
                    found = true;
                    break;
                }
            }
            if !found {
                self.globals.push(Global {
                    name,
                    value: pg.value,
                });
            }

            // If the persistent global is an object, re-key its properties
            // from the old pool to the new compiler pool.
            if pg.value.is_object() {
                if let Some(obj) = self.heap.get_mut(pg.value.as_object_index()) {
                    obj.rekey_properties(old_pool, strings);
                }
            }
        }
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
            closure_obj: None,
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

    fn read_byte(&mut self, code: &CodeBlock) -> u8 {
        let fi = self.frames.len() - 1;
        let ip = self.frames[fi].ip;
        self.frames[fi].ip += 1;
        code.code[ip]
    }

    fn read_u16_advance(&mut self, code: &CodeBlock) -> u16 {
        let fi = self.frames.len() - 1;
        let ip = self.frames[fi].ip;
        self.frames[fi].ip += 2;
        code.read_u16(ip)
    }

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
            let result = self.run_step(main, strings, functions);
            match result {
                Ok(StepResult::Continue) => continue,
                Ok(StepResult::Return(val)) => return Ok(val),
                Err(err) => {
                    // Try to route the error through an exception handler
                    self.handle_runtime_error(err, strings)?;
                    // If handle_runtime_error returned Ok, we caught it; continue
                }
            }
        }
    }

    fn run_step(
        &mut self,
        main: &CodeBlock,
        strings: &mut StringPool,
        functions: &[CodeBlock],
    ) -> Result<StepResult, JsError> {
        let fi = self.frames.len() - 1;
        let code = self.get_code(fi, main, functions);

        if self.frames[fi].ip >= code.code.len() {
            if self.frames.len() == 1 {
                return Ok(StepResult::Return(if self.stack.is_empty() {
                    JsValue::undefined()
                } else {
                    *self.stack.last().unwrap()
                }));
            }
            let result = self.stack.pop().unwrap_or(JsValue::undefined());
            let old_base = self.frames[fi].base;
            self.frames.pop();
            self.stack.truncate(old_base);
            self.stack.push(result);
            return Ok(StepResult::Continue);
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
                        Constant::Function(fi) => {
                            // Create a function object on the heap
                            let obj = JsObject::new_function(*fi);
                            let obj_idx = self.heap.alloc(obj);
                            JsValue::object(obj_idx)
                        }
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
                    let type_name = if a.is_object() {
                        // Check if it's a function or closure
                        if let Some(obj) = self.heap.get(a.as_object_index()) {
                            if matches!(obj.kind, ObjectKind::Function(_) | ObjectKind::Closure(_, _) | ObjectKind::NativeFunction(_)) {
                                "function"
                            } else {
                                "object"
                            }
                        } else {
                            a.type_name()
                        }
                    } else {
                        a.type_name()
                    };
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
                    self.do_call(argc, strings, functions)?;
                }

                Op::Return => {
                    let result = self.pop();

                    if self.frames.len() <= 1 {
                        return Ok(StepResult::Return(result));
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

                // --- Object operations ---

                Op::CreateObject => {
                    let obj = JsObject::new();
                    let idx = self.heap.alloc(obj);
                    self.push(JsValue::object(idx))?;
                }

                Op::SetProperty => {
                    let value = self.pop();
                    let key = self.pop();
                    // Object is still on the stack (peek, don't pop)
                    let obj_val = *self.stack.last().ok_or_else(|| {
                        JsError::internal("SetProperty: empty stack")
                    })?;
                    if !obj_val.is_object() {
                        return Err(JsError::type_error("cannot set property of non-object"));
                    }
                    let key_sid = if key.is_string() {
                        key.as_string_id()
                    } else {
                        let s = self.value_to_string(key, strings);
                        strings.intern(&s)
                    };
                    self.heap
                        .get_mut(obj_val.as_object_index())
                        .ok_or_else(|| JsError::internal("SetProperty: invalid object"))?
                        .set(key_sid, value);
                }

                Op::GetProperty => {
                    let key = self.pop();
                    let obj_val = self.pop();
                    let value = self.get_property_value(obj_val, key, strings)?;
                    self.push(value)?;
                }

                Op::SetPropertyConst => {
                    let name_idx = self.read_u16_advance(code) as usize;
                    let key_sid = match &code.constants[name_idx] {
                        Constant::String(sid) => StringId(*sid),
                        _ => return Err(JsError::internal("SetPropertyConst: expected string")),
                    };
                    let value = self.pop();
                    let obj_val = *self.stack.last().ok_or_else(|| {
                        JsError::internal("SetPropertyConst: empty stack")
                    })?;
                    if !obj_val.is_object() {
                        return Err(JsError::type_error("cannot set property of non-object"));
                    }
                    self.heap
                        .get_mut(obj_val.as_object_index())
                        .ok_or_else(|| JsError::internal("SetPropertyConst: invalid object"))?
                        .set(key_sid, value);
                }

                Op::GetPropertyConst => {
                    let name_idx = self.read_u16_advance(code) as usize;
                    let key_sid = match &code.constants[name_idx] {
                        Constant::String(sid) => StringId(*sid),
                        _ => return Err(JsError::internal("GetPropertyConst: expected string")),
                    };
                    let obj_val = self.pop();
                    let value = self.get_named_property(obj_val, key_sid, strings)?;
                    self.push(value)?;
                }

                // --- Array operations ---

                Op::CreateArray => {
                    let count = self.read_u16_advance(code) as usize;
                    let mut arr = JsObject::new_array();
                    let start = self.stack.len() - count;
                    for i in start..self.stack.len() {
                        arr.push(self.stack[i]);
                    }
                    for _ in 0..count {
                        self.pop();
                    }
                    let idx = self.heap.alloc(arr);
                    self.push(JsValue::object(idx))?;
                }

                Op::GetIndex => {
                    let index = self.pop();
                    let obj_val = self.pop();

                    if !obj_val.is_object() {
                        self.push(JsValue::undefined())?;
                    } else {
                        let idx = index.to_number() as u32;
                        let value = self
                            .heap
                            .get(obj_val.as_object_index())
                            .map(|o| o.get_index(idx))
                            .unwrap_or(JsValue::undefined());
                        self.push(value)?;
                    }
                }

                Op::SetIndex => {
                    let value = self.pop();
                    let index = self.pop();
                    // Object stays on stack
                    let obj_val = *self.stack.last().ok_or_else(|| {
                        JsError::internal("SetIndex: empty stack")
                    })?;
                    if !obj_val.is_object() {
                        return Err(JsError::type_error("cannot set index of non-object"));
                    }
                    let idx = index.to_number() as u32;
                    self.heap
                        .get_mut(obj_val.as_object_index())
                        .ok_or_else(|| JsError::internal("SetIndex: invalid object"))?
                        .set_index(idx, value);
                }

                Op::CallMethod => {
                    let name_idx = self.read_u16_advance(code) as usize;
                    let argc = self.read_u8_advance(code) as usize;
                    let key_sid = match &code.constants[name_idx] {
                        Constant::String(sid) => StringId(*sid),
                        _ => return Err(JsError::internal("CallMethod: expected string")),
                    };
                    let method_name = String::from(strings.get(key_sid));

                    // Collect arguments
                    let args_start = self.stack.len() - argc;
                    let args: Vec<JsValue> = self.stack[args_start..].to_vec();
                    for _ in 0..argc {
                        self.pop();
                    }
                    let obj_val = self.pop();

                    // Try built-in methods first
                    match self.call_builtin_method(obj_val, &method_name, &args, strings) {
                        Ok(result) => {
                            self.push(result)?;
                        }
                        Err(_) => {
                            // Fall back to user-defined method (property that is a function)
                            if obj_val.is_object() {
                                let method_val = self.heap
                                    .get(obj_val.as_object_index())
                                    .ok_or_else(|| JsError::internal("CallMethod: invalid object"))?
                                    .get(key_sid);

                                if method_val.is_object() {
                                    // Push function then args, do_call
                                    self.push(method_val)?;
                                    for arg in &args {
                                        self.push(*arg)?;
                                    }
                                    self.do_call(args.len(), strings, functions)?;
                                    // Result will be placed by the run loop
                                } else {
                                    return Err(JsError::type_error(alloc::format!(
                                        "'{}' is not a function",
                                        &method_name
                                    )));
                                }
                            } else {
                                return Err(JsError::type_error(alloc::format!(
                                    "cannot call method '{}' on non-object",
                                    &method_name
                                )));
                            }
                        }
                    }
                }

                Op::CreateClosure => {
                    let const_idx = self.read_u16_advance(code) as usize;
                    let upvalue_count = self.read_u8_advance(code) as usize;

                    let func_idx = match &code.constants[const_idx] {
                        Constant::Function(fi) => *fi,
                        _ => return Err(JsError::internal("CreateClosure: expected function constant")),
                    };

                    let mut captured = Vec::with_capacity(upvalue_count);
                    for _ in 0..upvalue_count {
                        let is_local = self.read_u8_advance(code) != 0;
                        let hi = self.read_u8_advance(code) as u16;
                        let lo = self.read_u8_advance(code) as u16;
                        let index = ((hi << 8) | lo) as usize;

                        if is_local {
                            // Capture from current frame's local variables
                            let base = self.current_base();
                            let val = self.stack[base + index];
                            captured.push(val);
                        } else {
                            // Capture from current frame's closure upvalues
                            let closure_obj_idx = self.frames.last().unwrap().closure_obj;
                            if let Some(obj_idx) = closure_obj_idx {
                                if let Some(obj) = self.heap.get(obj_idx) {
                                    if let ObjectKind::Closure(_, ref upvals) = obj.kind {
                                        captured.push(
                                            upvals.get(index).copied().unwrap_or(JsValue::undefined()),
                                        );
                                    } else {
                                        captured.push(JsValue::undefined());
                                    }
                                } else {
                                    captured.push(JsValue::undefined());
                                }
                            } else {
                                captured.push(JsValue::undefined());
                            }
                        }
                    }

                    let obj = JsObject::new_closure(func_idx, captured);
                    let obj_idx = self.heap.alloc(obj);
                    self.push(JsValue::object(obj_idx))?;
                }

                Op::LoadUpvalue => {
                    let upval_idx = self.read_u16_advance(code) as usize;
                    let closure_obj_idx = self.frames.last().unwrap().closure_obj;
                    let val = if let Some(obj_idx) = closure_obj_idx {
                        if let Some(obj) = self.heap.get(obj_idx) {
                            if let ObjectKind::Closure(_, ref upvals) = obj.kind {
                                upvals.get(upval_idx).copied().unwrap_or(JsValue::undefined())
                            } else {
                                JsValue::undefined()
                            }
                        } else {
                            JsValue::undefined()
                        }
                    } else {
                        JsValue::undefined()
                    };
                    self.push(val)?;
                }

                Op::StoreUpvalue => {
                    let upval_idx = self.read_u16_advance(code) as usize;
                    let val = self.pop();
                    let closure_obj_idx = self.frames.last().unwrap().closure_obj;
                    if let Some(obj_idx) = closure_obj_idx {
                        if let Some(obj) = self.heap.get_mut(obj_idx) {
                            if let ObjectKind::Closure(_, ref mut upvals) = obj.kind {
                                if upval_idx < upvals.len() {
                                    upvals[upval_idx] = val;
                                }
                            }
                        }
                    }
                }

                Op::TryStart => {
                    let catch_addr = self.read_u16_advance(code);
                    let _finally_addr = self.read_u16_advance(code);
                    self.exception_handlers.push(ExceptionHandler {
                        catch_addr,
                        stack_depth: self.stack.len(),
                        frame_depth: self.frames.len(),
                    });
                }

                Op::TryEnd => {
                    self.exception_handlers.pop();
                }

                Op::Throw => {
                    let error_val = self.pop();
                    if let Some(handler) = self.exception_handlers.pop() {
                        // Restore stack to depth at TryStart
                        self.stack.truncate(handler.stack_depth);
                        // Unwind call frames if needed
                        self.frames.truncate(handler.frame_depth);
                        // Push error value for catch block
                        self.push(error_val)?;
                        // Jump to catch block
                        self.set_ip(handler.catch_addr as usize);
                    } else {
                        // No handler - convert to string and return error
                        let msg = self.value_to_string(error_val, strings);
                        return Err(JsError::runtime(msg));
                    }
                }

                Op::Await => {
                    let val = self.pop();
                    if val.is_object() {
                        let resolved = {
                            let obj = self.heap.get(val.as_object_index());
                            match obj {
                                Some(obj) => match &obj.kind {
                                    ObjectKind::Promise(PromiseState::Fulfilled(v)) => Ok(*v),
                                    ObjectKind::Promise(PromiseState::Rejected(v)) => {
                                        let msg = self.value_to_string(*v, strings);
                                        Err(JsError::runtime(msg))
                                    }
                                    ObjectKind::Promise(PromiseState::Pending) => {
                                        Err(JsError::runtime("await: Promise is still pending"))
                                    }
                                    _ => Ok(val), // Not a promise, pass through
                                },
                                None => Ok(val),
                            }
                        };
                        match resolved {
                            Ok(v) => self.push(v)?,
                            Err(e) => return Err(e),
                        }
                    } else {
                        // Not an object - pass through (await on non-promise returns the value)
                        self.push(val)?;
                    }
                }

                Op::CallSpread => {
                    // Stack: [func, args_array]
                    let args_val = self.pop();
                    // Expand array elements onto the stack as individual args
                    let argc = if args_val.is_object() {
                        let obj = self.heap.get(args_val.as_object_index())
                            .ok_or_else(|| JsError::type_error("spread: expected array"))?;
                        let elems: Vec<JsValue> = obj.elements().to_vec();
                        let count = elems.len();
                        for elem in elems {
                            self.push(elem)?;
                        }
                        count
                    } else {
                        0
                    };
                    self.do_call(argc, strings, functions)?;
                }

                Op::Halt => {
                    return Ok(StepResult::Return(if self.stack.is_empty() {
                        JsValue::undefined()
                    } else {
                        self.pop()
                    }));
                }
            }

            Ok(StepResult::Continue)
    }

    /// Execute a function call with argc arguments on the stack.
    fn do_call(&mut self, argc: usize, strings: &mut StringPool, functions: &[CodeBlock]) -> JsResult<()> {
        let func_pos = self.stack.len() - argc - 1;
        let func_val = self.stack[func_pos];

        if !func_val.is_object() {
            return Err(JsError::type_error("not a function"));
        }

        let obj_idx = func_val.as_object_index();
        let call_kind = match self.heap.get(obj_idx) {
            Some(obj) => match &obj.kind {
                ObjectKind::Function(fi) => CallKind::Js(*fi as usize, None),
                ObjectKind::Closure(fi, _) => CallKind::Js(*fi as usize, Some(obj_idx)),
                ObjectKind::NativeFunction(ni) => CallKind::Native(*ni),
                _ => return Err(JsError::type_error("not a function")),
            },
            None => return Err(JsError::type_error("not a function")),
        };

        match call_kind {
            CallKind::Native(native_idx) => {
                // Collect args and call native function
                let args: Vec<JsValue> = self.stack[func_pos + 1..].to_vec();
                // Remove func + args from stack
                self.stack.truncate(func_pos);
                let func = self.native_functions[native_idx as usize].func;
                let result = func(&args, strings, &mut self.heap)?;
                self.push(result)?;
                Ok(())
            }
            CallKind::Js(func_index, closure_obj) => {
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
                    closure_obj,
                });

                Ok(())
            }
        }
    }

    /// Try to call a built-in method on a value. Returns Err if no built-in matches.
    fn call_builtin_method(
        &mut self,
        obj_val: JsValue,
        method_name: &str,
        args: &[JsValue],
        strings: &mut StringPool,
    ) -> JsResult<JsValue> {
        // String methods
        if obj_val.is_string() {
            let s = String::from(strings.get(obj_val.as_string_id()));
            return self.call_string_method(&s, method_name, args, strings);
        }

        if !obj_val.is_object() {
            return Err(JsError::type_error("not a built-in method"));
        }

        let obj_idx = obj_val.as_object_index();
        let obj_kind = {
            let obj = self.heap.get(obj_idx)
                .ok_or_else(|| JsError::internal("CallMethod: invalid object"))?;
            match &obj.kind {
                ObjectKind::Array => 0u8,
                ObjectKind::Promise(_) => 1,
                _ => 2,
            }
        };

        // Array built-in methods
        if obj_kind == 0 {
            return self.call_array_method(obj_idx, method_name, args, strings);
        }

        // Promise built-in methods
        if obj_kind == 1 {
            return self.call_promise_method(obj_idx, method_name, args, strings);
        }

        Err(JsError::type_error("not a built-in method"))
    }

    /// Call a built-in array method.
    fn call_array_method(
        &mut self,
        obj_idx: u32,
        method_name: &str,
        args: &[JsValue],
        strings: &mut StringPool,
    ) -> JsResult<JsValue> {
        match method_name {
            "push" => {
                let obj = self.heap.get_mut(obj_idx)
                    .ok_or_else(|| JsError::internal("array method: invalid object"))?;
                for arg in args {
                    obj.push(*arg);
                }
                Ok(JsValue::number(obj.elements_len() as f64))
            }
            "pop" => {
                let obj = self.heap.get_mut(obj_idx)
                    .ok_or_else(|| JsError::internal("array method: invalid object"))?;
                Ok(obj.pop())
            }
            "indexOf" => {
                let search = args.first().copied().unwrap_or(JsValue::undefined());
                let obj = self.heap.get(obj_idx)
                    .ok_or_else(|| JsError::internal("array method: invalid object"))?;
                for (i, elem) in obj.elements().iter().enumerate() {
                    if elem.strict_equal(&search) {
                        return Ok(JsValue::number(i as f64));
                    }
                }
                Ok(JsValue::number(-1.0))
            }
            "join" => {
                let separator = if let Some(sep) = args.first() {
                    if sep.is_string() {
                        String::from(strings.get(sep.as_string_id()))
                    } else {
                        self.value_to_string(*sep, strings)
                    }
                } else {
                    String::from(",")
                };
                let obj = self.heap.get(obj_idx)
                    .ok_or_else(|| JsError::internal("array method: invalid object"))?;
                let mut parts = Vec::new();
                for elem in obj.elements() {
                    parts.push(self.value_to_string(*elem, strings));
                }
                let result = parts.join(&separator);
                let id = strings.intern(&result);
                Ok(JsValue::string(id))
            }
            "slice" => {
                let obj = self.heap.get(obj_idx)
                    .ok_or_else(|| JsError::internal("array method: invalid object"))?;
                let len = obj.elements_len() as i64;

                let start_raw = args.first().map(|v| v.to_number() as i64).unwrap_or(0);
                let end_raw = args.get(1).map(|v| v.to_number() as i64).unwrap_or(len);

                let start = if start_raw < 0 {
                    (len + start_raw).max(0) as usize
                } else {
                    (start_raw as usize).min(len as usize)
                };
                let end = if end_raw < 0 {
                    (len + end_raw).max(0) as usize
                } else {
                    (end_raw as usize).min(len as usize)
                };

                let elements: Vec<JsValue> = if start < end {
                    obj.elements()[start..end].to_vec()
                } else {
                    Vec::new()
                };

                let mut new_arr = JsObject::new_array();
                for elem in elements {
                    new_arr.push(elem);
                }
                let idx = self.heap.alloc(new_arr);
                Ok(JsValue::object(idx))
            }
            "includes" => {
                let search = args.first().copied().unwrap_or(JsValue::undefined());
                let obj = self.heap.get(obj_idx)
                    .ok_or_else(|| JsError::internal("array method: invalid object"))?;
                for elem in obj.elements() {
                    if elem.strict_equal(&search) {
                        return Ok(JsValue::number(1.0));
                    }
                }
                Ok(JsValue::number(0.0))
            }
            "reverse" => {
                let obj = self.heap.get(obj_idx)
                    .ok_or_else(|| JsError::internal("array method: invalid object"))?;
                let mut elements: Vec<JsValue> = obj.elements().to_vec();
                elements.reverse();
                let obj = self.heap.get_mut(obj_idx)
                    .ok_or_else(|| JsError::internal("array method: invalid object"))?;
                // Replace elements
                for (i, elem) in elements.into_iter().enumerate() {
                    obj.set_index(i as u32, elem);
                }
                Ok(JsValue::object(obj_idx))
            }
            "concat" => {
                let obj = self.heap.get(obj_idx)
                    .ok_or_else(|| JsError::internal("array method: invalid object"))?;
                let mut new_arr = JsObject::new_array();
                for elem in obj.elements() {
                    new_arr.push(*elem);
                }
                // Add elements from argument arrays
                for arg in args {
                    if arg.is_object() {
                        if let Some(arg_obj) = self.heap.get(arg.as_object_index()) {
                            if arg_obj.kind == ObjectKind::Array {
                                for elem in arg_obj.elements() {
                                    new_arr.push(*elem);
                                }
                                continue;
                            }
                        }
                    }
                    new_arr.push(*arg);
                }
                let idx = self.heap.alloc(new_arr);
                Ok(JsValue::object(idx))
            }
            _ => Err(JsError::type_error(alloc::format!(
                "'{}' is not a function",
                method_name
            ))),
        }
    }

    /// Call a built-in Promise method (.then, .catch, .finally).
    fn call_promise_method(
        &mut self,
        obj_idx: u32,
        method_name: &str,
        args: &[JsValue],
        _strings: &mut StringPool,
    ) -> JsResult<JsValue> {
        // Get the current promise state
        let state = {
            let obj = self.heap.get(obj_idx)
                .ok_or_else(|| JsError::internal("promise: invalid object"))?;
            match &obj.kind {
                ObjectKind::Promise(s) => s.clone(),
                _ => return Err(JsError::type_error("not a promise")),
            }
        };

        match method_name {
            "then" => {
                let on_fulfilled = args.first().copied().unwrap_or(JsValue::undefined());
                match state {
                    PromiseState::Fulfilled(val) => {
                        if on_fulfilled.is_object() {
                            // Call the onFulfilled handler
                            self.push(on_fulfilled)?;
                            self.push(val)?;
                            // We can't easily call do_call here because we're already
                            // in a method call. Instead, return a new promise with the
                            // result. For eagerly-evaluated promises, we use a simpler model:
                            // just apply the function immediately if possible.
                            // For now, return a fulfilled promise with the same value.
                            // Full .then() chaining requires running the callback.
                            // Let's do it the simple way: pop the args we pushed and
                            // return a promise wrapping the original value.
                            self.pop(); // val
                            self.pop(); // on_fulfilled

                            // Actually, let's properly call the handler using inline evaluation
                            // Since we can't recurse into do_call easily, we'll return
                            // a fulfilled promise. Users should use await for chaining.
                            let result_obj = JsObject::new_promise_fulfilled(val);
                            let idx = self.heap.alloc(result_obj);
                            Ok(JsValue::object(idx))
                        } else {
                            // No handler, pass through
                            let result_obj = JsObject::new_promise_fulfilled(val);
                            let idx = self.heap.alloc(result_obj);
                            Ok(JsValue::object(idx))
                        }
                    }
                    PromiseState::Rejected(val) => {
                        let on_rejected = args.get(1).copied().unwrap_or(JsValue::undefined());
                        if on_rejected.is_object() {
                            // Has a rejection handler, pass through as fulfilled
                            let result_obj = JsObject::new_promise_fulfilled(val);
                            let idx = self.heap.alloc(result_obj);
                            Ok(JsValue::object(idx))
                        } else {
                            // No rejection handler, propagate rejection
                            let result_obj = JsObject::new_promise_rejected(val);
                            let idx = self.heap.alloc(result_obj);
                            Ok(JsValue::object(idx))
                        }
                    }
                    PromiseState::Pending => {
                        let result_obj = JsObject::new_promise_pending();
                        let idx = self.heap.alloc(result_obj);
                        Ok(JsValue::object(idx))
                    }
                }
            }
            "catch" => {
                let on_rejected = args.first().copied().unwrap_or(JsValue::undefined());
                match state {
                    PromiseState::Rejected(_val) if on_rejected.is_object() => {
                        // Would call on_rejected with val. For now, return fulfilled with undefined.
                        let result_obj = JsObject::new_promise_fulfilled(JsValue::undefined());
                        let idx = self.heap.alloc(result_obj);
                        Ok(JsValue::object(idx))
                    }
                    PromiseState::Rejected(val) => {
                        let result_obj = JsObject::new_promise_rejected(val);
                        let idx = self.heap.alloc(result_obj);
                        Ok(JsValue::object(idx))
                    }
                    PromiseState::Fulfilled(val) => {
                        // Not rejected, pass through
                        let result_obj = JsObject::new_promise_fulfilled(val);
                        let idx = self.heap.alloc(result_obj);
                        Ok(JsValue::object(idx))
                    }
                    PromiseState::Pending => {
                        let result_obj = JsObject::new_promise_pending();
                        let idx = self.heap.alloc(result_obj);
                        Ok(JsValue::object(idx))
                    }
                }
            }
            "finally" => {
                // .finally() returns a promise that preserves the original state
                match state {
                    PromiseState::Fulfilled(val) => {
                        let result_obj = JsObject::new_promise_fulfilled(val);
                        let idx = self.heap.alloc(result_obj);
                        Ok(JsValue::object(idx))
                    }
                    PromiseState::Rejected(val) => {
                        let result_obj = JsObject::new_promise_rejected(val);
                        let idx = self.heap.alloc(result_obj);
                        Ok(JsValue::object(idx))
                    }
                    PromiseState::Pending => {
                        let result_obj = JsObject::new_promise_pending();
                        let idx = self.heap.alloc(result_obj);
                        Ok(JsValue::object(idx))
                    }
                }
            }
            _ => Err(JsError::type_error(alloc::format!(
                "Promise has no method '{}'", method_name
            ))),
        }
    }

    /// Call a built-in string method.
    fn call_string_method(
        &mut self,
        s: &str,
        method_name: &str,
        args: &[JsValue],
        strings: &mut StringPool,
    ) -> JsResult<JsValue> {
        match method_name {
            "indexOf" => {
                let search = if let Some(arg) = args.first() {
                    if arg.is_string() {
                        String::from(strings.get(arg.as_string_id()))
                    } else {
                        self.value_to_string(*arg, strings)
                    }
                } else {
                    return Ok(JsValue::number(-1.0));
                };
                match s.find(&search) {
                    Some(pos) => Ok(JsValue::number(pos as f64)),
                    None => Ok(JsValue::number(-1.0)),
                }
            }
            "includes" => {
                let search = if let Some(arg) = args.first() {
                    if arg.is_string() {
                        String::from(strings.get(arg.as_string_id()))
                    } else {
                        self.value_to_string(*arg, strings)
                    }
                } else {
                    return Ok(JsValue::number(0.0));
                };
                Ok(JsValue::number(if s.contains(&search) { 1.0 } else { 0.0 }))
            }
            "startsWith" => {
                let search = if let Some(arg) = args.first() {
                    if arg.is_string() {
                        String::from(strings.get(arg.as_string_id()))
                    } else {
                        self.value_to_string(*arg, strings)
                    }
                } else {
                    return Ok(JsValue::number(0.0));
                };
                Ok(JsValue::number(if s.starts_with(&search) { 1.0 } else { 0.0 }))
            }
            "endsWith" => {
                let search = if let Some(arg) = args.first() {
                    if arg.is_string() {
                        String::from(strings.get(arg.as_string_id()))
                    } else {
                        self.value_to_string(*arg, strings)
                    }
                } else {
                    return Ok(JsValue::number(0.0));
                };
                Ok(JsValue::number(if s.ends_with(&search) { 1.0 } else { 0.0 }))
            }
            "trim" => {
                let trimmed = s.trim();
                let id = strings.intern(trimmed);
                Ok(JsValue::string(id))
            }
            "toUpperCase" => {
                let upper = s.to_uppercase();
                let id = strings.intern(&upper);
                Ok(JsValue::string(id))
            }
            "toLowerCase" => {
                let lower = s.to_lowercase();
                let id = strings.intern(&lower);
                Ok(JsValue::string(id))
            }
            "slice" => {
                let len = s.len() as i64;
                let start_raw = args.first().map(|v| v.to_number() as i64).unwrap_or(0);
                let end_raw = args.get(1).map(|v| v.to_number() as i64).unwrap_or(len);

                let start = if start_raw < 0 {
                    (len + start_raw).max(0) as usize
                } else {
                    (start_raw as usize).min(len as usize)
                };
                let end = if end_raw < 0 {
                    (len + end_raw).max(0) as usize
                } else {
                    (end_raw as usize).min(len as usize)
                };

                let sliced = if start < end {
                    &s[start..end]
                } else {
                    ""
                };
                let id = strings.intern(sliced);
                Ok(JsValue::string(id))
            }
            "split" => {
                let separator = if let Some(arg) = args.first() {
                    if arg.is_string() {
                        String::from(strings.get(arg.as_string_id()))
                    } else {
                        self.value_to_string(*arg, strings)
                    }
                } else {
                    // No separator: return array with entire string
                    let mut arr = JsObject::new_array();
                    let id = strings.intern(s);
                    arr.push(JsValue::string(id));
                    let idx = self.heap.alloc(arr);
                    return Ok(JsValue::object(idx));
                };

                let mut arr = JsObject::new_array();
                for part in s.split(&separator) {
                    let id = strings.intern(part);
                    arr.push(JsValue::string(id));
                }
                let idx = self.heap.alloc(arr);
                Ok(JsValue::object(idx))
            }
            "charAt" => {
                let index = args.first().map(|v| v.to_number() as usize).unwrap_or(0);
                let ch = s.chars().nth(index).map(|c| {
                    let mut buf = [0u8; 4];
                    let encoded = c.encode_utf8(&mut buf);
                    String::from(encoded)
                }).unwrap_or_default();
                let id = strings.intern(&ch);
                Ok(JsValue::string(id))
            }
            "replace" => {
                let search = if let Some(arg) = args.first() {
                    if arg.is_string() {
                        String::from(strings.get(arg.as_string_id()))
                    } else {
                        self.value_to_string(*arg, strings)
                    }
                } else {
                    return Ok(JsValue::string(strings.intern(s)));
                };
                let replacement = if let Some(arg) = args.get(1) {
                    if arg.is_string() {
                        String::from(strings.get(arg.as_string_id()))
                    } else {
                        self.value_to_string(*arg, strings)
                    }
                } else {
                    String::from("undefined")
                };
                // Only replace first occurrence (like JS)
                let result = s.replacen(&search, &replacement, 1);
                let id = strings.intern(&result);
                Ok(JsValue::string(id))
            }
            _ => Err(JsError::type_error(alloc::format!(
                "'{}' is not a function",
                method_name
            ))),
        }
    }

    /// Get a property from an object, handling built-in properties like .length.
    fn get_property_value(
        &self,
        obj_val: JsValue,
        key: JsValue,
        strings: &StringPool,
    ) -> JsResult<JsValue> {
        if !obj_val.is_object() {
            // String.length
            if obj_val.is_string() && key.is_string() {
                let key_str = strings.get(key.as_string_id());
                if key_str == "length" {
                    let s = strings.get(obj_val.as_string_id());
                    return Ok(JsValue::number(s.len() as f64));
                }
            }
            return Ok(JsValue::undefined());
        }

        let key_sid = if key.is_string() {
            key.as_string_id()
        } else {
            // Numeric index
            let idx = key.to_number() as u32;
            let value = self
                .heap
                .get(obj_val.as_object_index())
                .map(|o| o.get_index(idx))
                .unwrap_or(JsValue::undefined());
            return Ok(value);
        };

        self.get_named_property(obj_val, key_sid, strings)
    }

    /// Get a named property, handling built-in properties.
    fn get_named_property(
        &self,
        obj_val: JsValue,
        key_sid: StringId,
        strings: &StringPool,
    ) -> JsResult<JsValue> {
        if !obj_val.is_object() {
            // String.length
            if obj_val.is_string() {
                let key_str = strings.get(key_sid);
                if key_str == "length" {
                    let s = strings.get(obj_val.as_string_id());
                    return Ok(JsValue::number(s.len() as f64));
                }
            }
            return Ok(JsValue::undefined());
        }

        let obj = self
            .heap
            .get(obj_val.as_object_index())
            .ok_or_else(|| JsError::internal("GetProperty: invalid object"))?;

        let key_str = strings.get(key_sid);

        // Built-in array properties
        if obj.kind == ObjectKind::Array {
            match key_str {
                "length" => return Ok(JsValue::number(obj.elements_len() as f64)),
                _ => {}
            }
        }

        // Regular property lookup
        let value = obj.get(key_sid);
        Ok(value)
    }

    /// Handle a runtime error: if there's an active exception handler, route
    /// the error to its catch block by pushing the error message as a JS value
    /// and jumping to the catch address. Returns Ok(()) if caught, Err if not.
    fn handle_runtime_error(
        &mut self,
        error: JsError,
        strings: &mut StringPool,
    ) -> JsResult<()> {
        if let Some(handler) = self.exception_handlers.pop() {
            self.stack.truncate(handler.stack_depth);
            self.frames.truncate(handler.frame_depth);
            let msg_id = strings.intern(&error.message);
            self.stack.push(JsValue::string(msg_id));
            self.set_ip(handler.catch_addr as usize);
            Ok(())
        } else {
            Err(error)
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
        } else if value.is_object() {
            if let Some(obj) = self.heap.get(value.as_object_index()) {
                if obj.kind == ObjectKind::Array {
                    // Format array as comma-separated elements
                    let mut parts = Vec::new();
                    for elem in obj.elements() {
                        parts.push(self.value_to_string(*elem, strings));
                    }
                    return parts.join(",");
                }
            }
            String::from("[object Object]")
        } else {
            value.to_number_string()
        }
    }

    /// Clear all GC roots (stack, frames, globals). Used for testing.
    pub fn clear_roots(&mut self) {
        self.stack.clear();
        self.frames.clear();
        self.globals.clear();
    }

    /// Run garbage collection (mark-sweep).
    ///
    /// Marks all objects reachable from roots (stack, globals, call frames),
    /// then frees unreachable objects. Returns the number of objects freed.
    pub fn gc(&mut self) -> usize {
        // Phase 1: Clear all marks
        self.heap.unmark_all();

        // Phase 2: Mark from roots

        // Mark objects on the value stack
        for val in &self.stack {
            if val.is_object() {
                self.heap.mark(val.as_object_index());
            }
        }

        // Mark objects in globals
        for g in &self.globals {
            if g.value.is_object() {
                self.heap.mark(g.value.as_object_index());
            }
        }

        // Mark closure objects referenced by call frames
        for frame in &self.frames {
            if let Some(obj_idx) = frame.closure_obj {
                self.heap.mark(obj_idx);
            }
        }

        // Phase 3: Sweep unreachable objects
        self.heap.sweep()
    }
}
