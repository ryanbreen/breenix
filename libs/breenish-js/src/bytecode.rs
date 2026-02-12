//! Bytecode definitions for the breenish-js VM.
//!
//! The VM uses a stack-based architecture with bytecode instructions
//! encoded as variable-length sequences in a `Vec<u8>`.

use alloc::string::String;
use alloc::vec::Vec;

/// Bytecode opcodes.
///
/// Each opcode is a single byte, optionally followed by operands.
/// Operand sizes are documented per-opcode.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Push a constant from the constant pool onto the stack.
    /// Operand: u16 (constant pool index)
    LoadConst = 0,

    /// Load a local variable onto the stack.
    /// Operand: u16 (local slot index)
    LoadLocal = 1,

    /// Store the top of stack into a local variable.
    /// Operand: u16 (local slot index)
    StoreLocal = 2,

    /// Load a global variable onto the stack.
    /// Operand: u16 (global name index in constant pool)
    LoadGlobal = 3,

    /// Store the top of stack into a global variable.
    /// Operand: u16 (global name index in constant pool)
    StoreGlobal = 4,

    // --- Arithmetic ---

    /// Pop two values, push their sum.
    Add = 10,
    /// Pop two values, push their difference.
    Sub = 11,
    /// Pop two values, push their product.
    Mul = 12,
    /// Pop two values, push their quotient.
    Div = 13,
    /// Pop two values, push their remainder.
    Mod = 14,
    /// Pop one value, push its negation.
    Negate = 15,

    // --- Comparison ---

    /// Pop two values, push true if equal (==).
    Equal = 20,
    /// Pop two values, push true if not equal (!=).
    NotEqual = 21,
    /// Pop two values, push true if strict equal (===).
    StrictEqual = 22,
    /// Pop two values, push true if strict not equal (!==).
    StrictNotEqual = 23,
    /// Pop two values, push true if a < b.
    LessThan = 24,
    /// Pop two values, push true if a > b.
    GreaterThan = 25,
    /// Pop two values, push true if a <= b.
    LessEqual = 26,
    /// Pop two values, push true if a >= b.
    GreaterEqual = 27,

    // --- Logical ---

    /// Pop one value, push logical NOT.
    Not = 30,
    /// Pop one value, push typeof string.
    TypeOf = 31,

    // --- Control flow ---

    /// Unconditional jump.
    /// Operand: u16 (absolute bytecode offset)
    Jump = 40,
    /// Jump if top of stack is falsy (pops the value).
    /// Operand: u16 (absolute bytecode offset)
    JumpIfFalse = 41,
    /// Jump if top of stack is truthy (pops the value).
    /// Operand: u16 (absolute bytecode offset)
    JumpIfTrue = 42,

    // --- Functions ---

    /// Call a function.
    /// Operand: u8 (argument count)
    /// Stack: [func, arg0, arg1, ...argN] -> [result]
    Call = 50,
    /// Return from function. Pops return value from stack.
    Return = 51,

    // --- Stack manipulation ---

    /// Pop and discard the top of stack.
    Pop = 60,
    /// Duplicate the top of stack.
    Dup = 61,

    // --- Built-in operations ---

    /// Call the built-in print function.
    /// Operand: u8 (argument count)
    Print = 70,

    // --- String operations ---

    /// Concatenate top two stack values as strings.
    Concat = 80,

    // --- Object operations ---

    /// Create a new empty object and push it.
    CreateObject = 90,
    /// Pop value and key (string), peek object, set property.
    /// Stack: [obj, key, value] -> [obj]
    /// Operand: none (key is on stack as a string constant index via LoadConst)
    SetProperty = 91,
    /// Pop key (string) and object, push property value.
    /// Stack: [obj, key] -> [value]
    GetProperty = 92,
    /// Set a property using a constant key name.
    /// Operand: u16 (constant pool index for property name string)
    /// Stack: [obj, value] -> [obj]
    SetPropertyConst = 93,
    /// Get a property using a constant key name.
    /// Operand: u16 (constant pool index for property name string)
    /// Stack: [obj] -> [value]
    GetPropertyConst = 94,

    // --- Array operations ---

    /// Create a new array with N elements from the stack.
    /// Operand: u16 (element count)
    /// Stack: [elem0, elem1, ..., elemN-1] -> [array]
    CreateArray = 100,
    /// Get an indexed element.
    /// Stack: [obj, index] -> [value]
    GetIndex = 101,
    /// Set an indexed element.
    /// Stack: [obj, index, value] -> [obj]
    SetIndex = 102,

    /// Call a method on an object.
    /// Operand: u16 (method name constant pool index), u8 (argument count)
    /// Stack: [obj, arg0, arg1, ...argN] -> [result]
    CallMethod = 103,

    // --- Closure operations ---

    /// Create a closure (function + captured environment).
    /// Operand: u16 (function constant pool index), u8 (upvalue count)
    CreateClosure = 110,
    /// Load an upvalue (captured variable from enclosing scope).
    /// Operand: u16 (upvalue index)
    LoadUpvalue = 111,
    /// Store into an upvalue.
    /// Operand: u16 (upvalue index)
    StoreUpvalue = 112,

    // --- Exception handling ---

    /// Start a try block. Pushes an exception handler.
    /// Operand: u16 (catch block address), u16 (finally block address, 0xFFFF if none)
    TryStart = 120,
    /// End a try block. Pops the exception handler.
    TryEnd = 121,
    /// Throw the top-of-stack value as an exception.
    Throw = 122,

    // --- Spread operations ---

    /// Call a function with spread arguments.
    /// Stack: [func, array] -> [result]
    /// The array's elements become the function's arguments.
    CallSpread = 130,

    /// Await a Promise value.
    /// Stack: [value] -> [resolved_value]
    /// If the value is a fulfilled Promise, pushes the resolved value.
    /// If it's a rejected Promise, throws the rejection reason.
    /// If it's not a Promise, pushes the value unchanged.
    Await = 131,

    /// Wrap the top-of-stack value in a fulfilled Promise.
    /// Stack: [value] -> [Promise(fulfilled(value))]
    /// Used by async functions to wrap their return value.
    WrapPromise = 132,

    /// Halt execution.
    Halt = 255,
}

impl Op {
    /// Convert a byte to an opcode.
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Op::LoadConst),
            1 => Some(Op::LoadLocal),
            2 => Some(Op::StoreLocal),
            3 => Some(Op::LoadGlobal),
            4 => Some(Op::StoreGlobal),
            10 => Some(Op::Add),
            11 => Some(Op::Sub),
            12 => Some(Op::Mul),
            13 => Some(Op::Div),
            14 => Some(Op::Mod),
            15 => Some(Op::Negate),
            20 => Some(Op::Equal),
            21 => Some(Op::NotEqual),
            22 => Some(Op::StrictEqual),
            23 => Some(Op::StrictNotEqual),
            24 => Some(Op::LessThan),
            25 => Some(Op::GreaterThan),
            26 => Some(Op::LessEqual),
            27 => Some(Op::GreaterEqual),
            30 => Some(Op::Not),
            31 => Some(Op::TypeOf),
            40 => Some(Op::Jump),
            41 => Some(Op::JumpIfFalse),
            42 => Some(Op::JumpIfTrue),
            50 => Some(Op::Call),
            51 => Some(Op::Return),
            60 => Some(Op::Pop),
            61 => Some(Op::Dup),
            70 => Some(Op::Print),
            80 => Some(Op::Concat),
            90 => Some(Op::CreateObject),
            91 => Some(Op::SetProperty),
            92 => Some(Op::GetProperty),
            93 => Some(Op::SetPropertyConst),
            94 => Some(Op::GetPropertyConst),
            100 => Some(Op::CreateArray),
            101 => Some(Op::GetIndex),
            102 => Some(Op::SetIndex),
            103 => Some(Op::CallMethod),
            110 => Some(Op::CreateClosure),
            111 => Some(Op::LoadUpvalue),
            112 => Some(Op::StoreUpvalue),
            120 => Some(Op::TryStart),
            121 => Some(Op::TryEnd),
            122 => Some(Op::Throw),
            130 => Some(Op::CallSpread),
            131 => Some(Op::Await),
            132 => Some(Op::WrapPromise),
            255 => Some(Op::Halt),
            _ => None,
        }
    }
}

/// A constant value in the constant pool.
#[derive(Debug, Clone)]
pub enum Constant {
    /// A number constant.
    Number(f64),
    /// An interned string constant (by pool index).
    String(u32),
    /// A function constant (index into function table).
    Function(u32),
}

/// A compiled block of bytecode.
#[derive(Debug)]
pub struct CodeBlock {
    /// The bytecode instructions.
    pub code: Vec<u8>,
    /// The constant pool.
    pub constants: Vec<Constant>,
    /// The number of local variable slots needed.
    pub local_count: u16,
    /// Name of this code block (for debugging).
    pub name: String,
}

impl CodeBlock {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            code: Vec::new(),
            constants: Vec::new(),
            local_count: 0,
            name: name.into(),
        }
    }

    /// Emit a single byte.
    pub fn emit(&mut self, byte: u8) {
        self.code.push(byte);
    }

    /// Emit an opcode.
    pub fn emit_op(&mut self, op: Op) {
        self.code.push(op as u8);
    }

    /// Emit an opcode followed by a u16 operand.
    pub fn emit_op_u16(&mut self, op: Op, operand: u16) {
        self.code.push(op as u8);
        self.code.push((operand >> 8) as u8);
        self.code.push(operand as u8);
    }

    /// Emit an opcode followed by a u8 operand.
    pub fn emit_op_u8(&mut self, op: Op, operand: u8) {
        self.code.push(op as u8);
        self.code.push(operand);
    }

    /// Emit an opcode followed by two u16 operands.
    pub fn emit_op_u16_u16(&mut self, op: Op, op1: u16, op2: u16) {
        self.code.push(op as u8);
        self.code.push((op1 >> 8) as u8);
        self.code.push(op1 as u8);
        self.code.push((op2 >> 8) as u8);
        self.code.push(op2 as u8);
    }

    /// Emit an opcode followed by a u16 and u8 operand.
    pub fn emit_op_u16_u8(&mut self, op: Op, operand16: u16, operand8: u8) {
        self.code.push(op as u8);
        self.code.push((operand16 >> 8) as u8);
        self.code.push(operand16 as u8);
        self.code.push(operand8);
    }

    /// Get the current bytecode offset (for jump targets).
    pub fn current_offset(&self) -> usize {
        self.code.len()
    }

    /// Emit a jump with a placeholder target. Returns the offset of the placeholder.
    pub fn emit_jump(&mut self, op: Op) -> usize {
        self.code.push(op as u8);
        let offset = self.code.len();
        self.code.push(0);
        self.code.push(0);
        offset
    }

    /// Patch a previously emitted jump placeholder with the actual target.
    pub fn patch_jump(&mut self, offset: usize, target: usize) {
        let target = target as u16;
        self.code[offset] = (target >> 8) as u8;
        self.code[offset + 1] = target as u8;
    }

    /// Add a constant to the pool and return its index.
    pub fn add_constant(&mut self, constant: Constant) -> u16 {
        let index = self.constants.len();
        self.constants.push(constant);
        index as u16
    }

    /// Add a number constant.
    pub fn add_number(&mut self, value: f64) -> u16 {
        // Check for existing constant
        for (i, c) in self.constants.iter().enumerate() {
            if let Constant::Number(n) = c {
                if *n == value || (n.is_nan() && value.is_nan()) {
                    return i as u16;
                }
            }
        }
        self.add_constant(Constant::Number(value))
    }

    /// Add a string constant (by string pool index).
    pub fn add_string(&mut self, string_id: u32) -> u16 {
        for (i, c) in self.constants.iter().enumerate() {
            if let Constant::String(s) = c {
                if *s == string_id {
                    return i as u16;
                }
            }
        }
        self.add_constant(Constant::String(string_id))
    }

    /// Read a u16 operand at the given offset.
    pub fn read_u16(&self, offset: usize) -> u16 {
        ((self.code[offset] as u16) << 8) | (self.code[offset + 1] as u16)
    }

    /// Read a u8 operand at the given offset.
    pub fn read_u8(&self, offset: usize) -> u8 {
        self.code[offset]
    }

    /// Disassemble the bytecode for debugging.
    #[cfg(feature = "std")]
    pub fn disassemble(&self) -> String {
        use alloc::format;
        let mut out = String::new();
        let mut ip = 0;
        while ip < self.code.len() {
            let op_byte = self.code[ip];
            let op = Op::from_byte(op_byte);
            match op {
                Some(Op::LoadConst) => {
                    let idx = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: LoadConst {}\n", ip, idx));
                    ip += 3;
                }
                Some(Op::LoadLocal) => {
                    let idx = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: LoadLocal {}\n", ip, idx));
                    ip += 3;
                }
                Some(Op::StoreLocal) => {
                    let idx = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: StoreLocal {}\n", ip, idx));
                    ip += 3;
                }
                Some(Op::LoadGlobal) => {
                    let idx = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: LoadGlobal {}\n", ip, idx));
                    ip += 3;
                }
                Some(Op::StoreGlobal) => {
                    let idx = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: StoreGlobal {}\n", ip, idx));
                    ip += 3;
                }
                Some(Op::Jump) | Some(Op::JumpIfFalse) | Some(Op::JumpIfTrue) => {
                    let target = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: {:?} -> {}\n", ip, op.unwrap(), target));
                    ip += 3;
                }
                Some(Op::Call) | Some(Op::Print) => {
                    let argc = self.read_u8(ip + 1);
                    out.push_str(&format!("{:04}: {:?} argc={}\n", ip, op.unwrap(), argc));
                    ip += 2;
                }
                Some(Op::CallMethod) => {
                    let name_idx = self.read_u16(ip + 1);
                    let argc = self.read_u8(ip + 3);
                    out.push_str(&format!("{:04}: CallMethod name={} argc={}\n", ip, name_idx, argc));
                    ip += 4;
                }
                Some(Op::CreateClosure) => {
                    let const_idx = self.read_u16(ip + 1);
                    let upvalue_count = self.read_u8(ip + 3);
                    out.push_str(&format!(
                        "{:04}: CreateClosure func={} upvalues={}\n",
                        ip, const_idx, upvalue_count
                    ));
                    ip += 4 + (upvalue_count as usize) * 3;
                }
                Some(Op::LoadUpvalue) => {
                    let idx = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: LoadUpvalue {}\n", ip, idx));
                    ip += 3;
                }
                Some(Op::StoreUpvalue) => {
                    let idx = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: StoreUpvalue {}\n", ip, idx));
                    ip += 3;
                }
                Some(Op::TryStart) => {
                    let catch_addr = self.read_u16(ip + 1);
                    let finally_addr = self.read_u16(ip + 3);
                    out.push_str(&format!(
                        "{:04}: TryStart catch={} finally={}\n",
                        ip, catch_addr, finally_addr
                    ));
                    ip += 5;
                }
                Some(Op::SetPropertyConst) | Some(Op::GetPropertyConst) => {
                    let idx = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: {:?} {}\n", ip, op.unwrap(), idx));
                    ip += 3;
                }
                Some(Op::CreateArray) => {
                    let count = self.read_u16(ip + 1);
                    out.push_str(&format!("{:04}: CreateArray count={}\n", ip, count));
                    ip += 3;
                }
                Some(op) => {
                    out.push_str(&format!("{:04}: {:?}\n", ip, op));
                    ip += 1;
                }
                None => {
                    out.push_str(&format!("{:04}: <unknown 0x{:02x}>\n", ip, op_byte));
                    ip += 1;
                }
            }
        }
        out
    }
}
