//! The catalogue's programs as SM4 token streams for SVGA3D DX shaders.
//!
//! Breenix writes each rendering program once, in the TGSI text form (`virgl::encode`), from
//! the TGSI documentation. This translator maps that text, instruction by instruction, onto
//! the shader-model-4 token format Microsoft publishes (`D3D12TokenizedProgramFormat.hpp`,
//! the D3D10_SB definitions): the program's version and length tokens, its declarations,
//! one SM4 instruction per TGSI instruction, and a closing `ret`. VMware's device takes the
//! bare token stream followed by its signature header and entries (`svga3d_dx.h`,
//! `SVGA3dDXSignatureHeader`), with no DXBC container (`docs/gui/gpu.md` section 1.6.3).
//! Each mapping names the TGSI instruction's documented meaning and the SM4 instruction
//! with the same per-component result; the provenance record lists the token facts.
//!
//! A program's constants reach it as fragment inputs, not a constant buffer: the device
//! refuses constant-buffer surfaces (provenance record, experiment V1). Each draw's
//! constants are one record of a per-instance vertex stream, which the vertex program
//! passes to the fragment program's inputs 1 to 8 with constant (flat) interpolation, so
//! `CONST[n]` reads input `1 + n` and each component arrives with its bits unchanged.
//!
//! The subset the catalogue uses is accepted. Other forms are refused, naming their line,
//! so a program the translator does not understand does not reach the device.

use alloc::vec::Vec;

/// Shader stages, as the version token and `SVGA3dShaderType` name them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Vertex,
    Pixel,
}

impl Stage {
    /// `SVGA3dShaderType` (`svga3d_types.h`): VS 1, PS 2.
    pub fn svga_type(self) -> u32 {
        match self {
            Stage::Vertex => 1,
            Stage::Pixel => 2,
        }
    }
}

/// A translated program: its tokens with the signature trailer, and how many of them are
/// the SM4 program (`DX_DEFINE_SHADER`'s byte size counts only those).
pub struct Program {
    pub stage: Stage,
    pub words: Vec<u32>,
    pub code_words: usize,
}

/// Why a text was refused: its 1-based line and what was wrong.
#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub what: &'static str,
}

/// SM4 opcodes (`D3D10_SB_OPCODE_TYPE`, numbered in the enumeration's order).
mod op {
    pub const ADD: u32 = 0;
    pub const AND: u32 = 1;
    pub const BREAK: u32 = 2;
    pub const DISCARD: u32 = 13;
    pub const ELSE: u32 = 18;
    pub const ENDIF: u32 = 21;
    pub const ENDLOOP: u32 = 22;
    pub const FTOI: u32 = 27;
    pub const FTOU: u32 = 28;
    pub const IADD: u32 = 30;
    pub const IF: u32 = 31;
    pub const IEQ: u32 = 32;
    pub const INE: u32 = 39;
    pub const ISHL: u32 = 41;
    pub const LD: u32 = 45;
    pub const LOOP: u32 = 48;
    pub const MOV: u32 = 54;
    pub const MUL: u32 = 56;
    pub const OR: u32 = 60;
    pub const RET: u32 = 62;
    pub const ULT: u32 = 79;
    pub const UMUL: u32 = 81;
    pub const USHR: u32 = 85;
    pub const UTOF: u32 = 86;
    pub const XOR: u32 = 87;
    pub const DCL_RESOURCE: u32 = 88;
    pub const DCL_INPUT: u32 = 95;
    pub const DCL_INPUT_PS: u32 = 98;
    pub const DCL_INPUT_PS_SIV: u32 = 100;
    pub const DCL_OUTPUT: u32 = 101;
    pub const DCL_OUTPUT_SIV: u32 = 103;
    pub const DCL_TEMPS: u32 = 104;
}

/// Operand types (`D3D10_SB_OPERAND_TYPE`).
mod file {
    pub const TEMP: u32 = 0;
    pub const INPUT: u32 = 1;
    pub const OUTPUT: u32 = 2;
    pub const IMMEDIATE32: u32 = 4;
    pub const RESOURCE: u32 = 7;
    pub const NULL: u32 = 13;
}

/// `D3D10_SB_NAME_POSITION`, the position system value.
const NAME_POSITION: u32 = 1;
/// `D3D10_SB_INTERPOLATION_MODE` 4, linear without perspective: TGSI's `LINEAR` on a
/// position input; mode 1, constant, for the constants.
const LINEAR_NOPERSPECTIVE: u32 = 4;
const CONSTANT: u32 = 1;
/// The constant vectors a draw's record carries, and so the most `CONST[n]` a program may
/// read: inputs 1 to `CONSTANTS`.
pub const CONSTANTS: u32 = 8;
/// `D3D10_SB_RESOURCE_DIMENSION_TEXTURE2D`.
const TEXTURE2D: u32 = 3;
/// `D3D10_SB_RETURN_TYPE_FLOAT` (5) in each of the four 4-bit fields: TGSI's `FLOAT` view.
const RETURN_FLOAT4: u32 = 0x5555;
/// `D3D10_SB_INSTRUCTION_TEST_NONZERO` in the opcode token's bit 18.
const TEST_NONZERO: u32 = 1 << 18;
/// `SVGADX_SIGNATURE_HEADER_VERSION_0`.
const SIGNATURE_VERSION: u32 = 0x08a9_2d12;

// Operand token 0 fields (D3D10_SB_OPERAND_*): component count in bits 0-1 (2 is four
// components), selection mode in bits 2-3 (0 mask, 1 swizzle, 2 select-1), the mask, swizzle
// or selected component from bit 4, the operand type from bit 12, the index dimension from
// bit 20 (each index an immediate 32-bit value).
const FOUR: u32 = 2;
const MODE_SWIZZLE: u32 = 1 << 2;
const MODE_SELECT1: u32 = 2 << 2;
const XYZW: u32 = 0xe4;

fn operand(kind: u32, dims: u32) -> u32 {
    (kind << 12) | (dims << 20)
}

/// A register a TGSI operand names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reg {
    Temp(u32),
    In(u32),
    Out(u32),
    Const(u32),
    Imm(u32),
    View(u32),
}

struct Src {
    reg: Reg,
    swizzle: [u8; 4],
}

struct Dst {
    reg: Reg,
    mask: u32,
}

struct Text<'a> {
    stage: Stage,
    consts: Option<u32>,
    temps: u32,
    views: Vec<u32>,
    imms: Vec<[u32; 4]>,
    body: Vec<(usize, &'a str)>,
}

/// Translate one TGSI program text (optionally NUL-terminated).
pub fn translate(text: &[u8]) -> Result<Program, Error> {
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    let text = core::str::from_utf8(&text[..end]).map_err(|_| Error { line: 0, what: "text is not UTF-8" })?;
    let t = parse(text)?;
    let mut w: Vec<u32> = Vec::new();
    // Version token (D3D10_SB_TOKENIZED_PROGRAM_TYPE in bits 16-31, major 4, minor 0) and
    // the length token, which counts the program's tokens, these two included.
    w.push(match t.stage {
        Stage::Pixel => 0x0000_0040,
        Stage::Vertex => 0x0001_0040,
    });
    w.push(0);
    if t.stage != Stage::Pixel {
        return Err(Error { line: 1, what: "only fragment programs are translated" });
    }
    for &v in &t.views {
        // dcl_resource_texture2d (float x4) t#v. The resource operand has no components: a
        // writemask there is refused by the device (section 1.6.3, item 5).
        inst(&mut w, op::DCL_RESOURCE | (TEXTURE2D << 11), &[operand(file::RESOURCE, 1), v, RETURN_FLOAT4]);
    }
    // dcl_input_ps_siv linear noperspective v0.xyzw, position; dcl_input_ps constant
    // v1.xyzw to v8.xyzw, the draw's constants; dcl_output o0.xyzw.
    inst(
        &mut w,
        op::DCL_INPUT_PS_SIV | (LINEAR_NOPERSPECTIVE << 11),
        &[FOUR | (0xf << 4) | operand(file::INPUT, 1), 0, NAME_POSITION],
    );
    for v in 1..=CONSTANTS {
        inst(&mut w, op::DCL_INPUT_PS | (CONSTANT << 11), &[FOUR | (0xf << 4) | operand(file::INPUT, 1), v]);
    }
    inst(&mut w, op::DCL_OUTPUT, &[FOUR | (0xf << 4) | operand(file::OUTPUT, 1), 0]);
    if t.temps > 0 {
        inst(&mut w, op::DCL_TEMPS, &[t.temps]);
    }
    for &(line, s) in &t.body {
        instruction(&mut w, &t, line, s)?;
    }
    inst(&mut w, op::RET, &[]);
    let code_words = w.len();
    w[1] = code_words as u32;
    signature(&mut w, NAME_POSITION, 0);
    Ok(Program { stage: t.stage, words: w, code_words })
}

/// VMware's signature trailer: registers 0 to `CONSTANTS` in, the first named `input`, and
/// for a vertex program the same registers out, the first named `output` (a fragment
/// program has one output, the colour target, which has no system-value name). Each entry's
/// component mask is 0xf.
fn signature(w: &mut Vec<u32>, input: u32, output: u32) {
    let inputs = CONSTANTS + 1;
    let outputs = if output == NAME_POSITION { inputs } else { 1 };
    w.extend_from_slice(&[SIGNATURE_VERSION, inputs, outputs, 0]);
    for r in 0..inputs {
        w.extend_from_slice(&[r, if r == 0 { input } else { 0 }, 0xf, 0, 0]);
    }
    for r in 0..outputs {
        w.extend_from_slice(&[r, if r == 0 { output } else { 0 }, 0xf, 0, 0]);
    }
}

/// The vertex program each pipeline binds a copy of: the vertex position (input 0) moves
/// to the position output, and the draw's constants (inputs 1 to `CONSTANTS`, from the
/// per-instance stream) to outputs 1 to `CONSTANTS`, the fragment program's inputs.
pub fn vertex() -> Program {
    let mut w: Vec<u32> = alloc::vec![0x0001_0040, 0];
    for r in 0..=CONSTANTS {
        inst(&mut w, op::DCL_INPUT, &[FOUR | (0xf << 4) | operand(file::INPUT, 1), r]);
    }
    inst(&mut w, op::DCL_OUTPUT_SIV, &[FOUR | (0xf << 4) | operand(file::OUTPUT, 1), 0, NAME_POSITION]);
    for r in 1..=CONSTANTS {
        inst(&mut w, op::DCL_OUTPUT, &[FOUR | (0xf << 4) | operand(file::OUTPUT, 1), r]);
    }
    for r in 0..=CONSTANTS {
        inst(
            &mut w,
            op::MOV,
            &[FOUR | (0xf << 4) | operand(file::OUTPUT, 1), r, FOUR | MODE_SWIZZLE | (XYZW << 4) | operand(file::INPUT, 1), r],
        );
    }
    inst(&mut w, op::RET, &[]);
    let code_words = w.len();
    w[1] = code_words as u32;
    signature(&mut w, 0, NAME_POSITION);
    Program { stage: Stage::Vertex, words: w, code_words }
}

/// Append an instruction token (opcode, its length in bits 24-30) and its operands.
fn inst(w: &mut Vec<u32>, opcode: u32, operands: &[u32]) {
    w.push(opcode | (((operands.len() + 1) as u32) << 24));
    w.extend_from_slice(operands);
}

fn parse(text: &str) -> Result<Text<'_>, Error> {
    let mut t = Text { stage: Stage::Pixel, consts: None, temps: 0, views: Vec::new(), imms: Vec::new(), body: Vec::new() };
    let mut header = false;
    let mut ended = false;
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        let err = |what| Error { line, what };
        if ended {
            return Err(err("text after END"));
        }
        if !header {
            t.stage = match s {
                "FRAG" => Stage::Pixel,
                "VERT" => Stage::Vertex,
                _ => return Err(err("the program does not start with FRAG or VERT")),
            };
            header = true;
            continue;
        }
        if s == "END" {
            ended = true;
        } else if let Some(rest) = s.strip_prefix("DCL ") {
            declaration(&mut t, rest).map_err(err)?;
        } else if s.starts_with("IMM[") {
            immediate(&mut t, s).map_err(err)?;
        } else {
            t.body.push((line, s));
        }
    }
    if !ended {
        return Err(Error { line: 0, what: "the program has no END" });
    }
    Ok(t)
}

/// The register range `[a]` or `[a..b]` after a file name, as (first, last).
fn range(s: &str) -> Option<(u32, u32, &str)> {
    let s = s.strip_prefix('[')?;
    let close = s.find(']')?;
    let inner = &s[..close];
    let rest = &s[close + 1..];
    match inner.split_once("..") {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?, rest)),
        None => {
            let a = inner.parse().ok()?;
            Some((a, a, rest))
        }
    }
}

fn declaration(t: &mut Text<'_>, s: &str) -> Result<(), &'static str> {
    let (name, rest) = s.split_at(s.find('[').ok_or("declaration without a register")?);
    let (first, last, tail) = range(rest).ok_or("malformed declaration range")?;
    let tail = tail.trim();
    match (name, t.stage) {
        ("IN", Stage::Pixel) if first == 0 && last == 0 && tail == ", POSITION, LINEAR" => {}
        ("IN", Stage::Vertex) if first == 0 && last == 0 && tail.is_empty() => {}
        ("OUT", Stage::Pixel) if first == 0 && last == 0 && tail == ", COLOR" => {}
        ("OUT", Stage::Vertex) if first == 0 && last == 0 && tail == ", POSITION" => {}
        ("CONST", _) if first == 0 && last < CONSTANTS && tail.is_empty() => t.consts = Some(last + 1),
        ("TEMP", _) if first == 0 && tail.is_empty() => t.temps = last + 1,
        // TXF ignores sampler state (the TGSI documentation, TXF); SM4's `ld` takes no sampler.
        ("SAMP", Stage::Pixel) if first == last && tail.is_empty() => {}
        ("SVIEW", Stage::Pixel) if first == last && tail == ", 2D, FLOAT" => t.views.push(first),
        _ => return Err("unsupported declaration"),
    }
    Ok(())
}

fn immediate(t: &mut Text<'_>, s: &str) -> Result<(), &'static str> {
    let (first, last, rest) = range(&s[3..]).ok_or("malformed immediate")?;
    if first != last || first as usize != t.imms.len() {
        return Err("immediates must be numbered in order");
    }
    let rest = rest.trim();
    let (kind, values) = rest.split_once(' ').ok_or("malformed immediate")?;
    if !matches!(kind, "UINT32" | "INT32" | "FLT32") {
        return Err("unsupported immediate type");
    }
    let values = values.trim().strip_prefix('{').and_then(|v| v.strip_suffix('}')).ok_or("malformed immediate values")?;
    let mut out = [0u32; 4];
    let mut n = 0;
    for v in values.split(',') {
        if n == 4 {
            return Err("more than four immediate values");
        }
        let v = v.trim();
        out[n] = match kind {
            "INT32" => v.parse::<i32>().map(|x| x as u32).map_err(|_| "malformed integer immediate")?,
            "UINT32" => v.parse::<u32>().map_err(|_| "malformed unsigned immediate")?,
            // FLT32 immediates would need a decimal-to-binary32 conversion; the catalogue
            // writes float constants as UINT32 bit patterns instead.
            _ => return Err("FLT32 immediates are not supported"),
        };
        n += 1;
    }
    if n != 4 {
        return Err("an immediate needs four values");
    }
    t.imms.push(out);
    Ok(())
}

fn register(s: &str) -> Result<(Reg, &str), &'static str> {
    let open = s.find('[').ok_or("operand without a register index")?;
    let (name, rest) = s.split_at(open);
    let (first, last, tail) = range(rest).ok_or("malformed operand index")?;
    if first != last {
        return Err("operand names a range");
    }
    let reg = match name {
        "TEMP" => Reg::Temp(first),
        "IN" => Reg::In(first),
        "OUT" => Reg::Out(first),
        "CONST" => Reg::Const(first),
        "IMM" => Reg::Imm(first),
        "SAMP" => Reg::View(first),
        _ => return Err("unsupported register file"),
    };
    Ok((reg, tail))
}

fn component(c: u8) -> Result<u8, &'static str> {
    match c {
        b'x' => Ok(0),
        b'y' => Ok(1),
        b'z' => Ok(2),
        b'w' => Ok(3),
        _ => Err("unknown component"),
    }
}

fn dst(s: &str) -> Result<Dst, &'static str> {
    let (reg, tail) = register(s)?;
    if !matches!(reg, Reg::Temp(_) | Reg::Out(_)) {
        return Err("destination is not a temporary or output");
    }
    let mask = match tail.strip_prefix('.') {
        None if tail.is_empty() => 0xf,
        Some(m) if !m.is_empty() => {
            let mut mask = 0u32;
            let mut prev: i32 = -1;
            for &c in m.as_bytes() {
                let k = component(c)? as i32;
                if k <= prev {
                    return Err("destination mask out of order");
                }
                prev = k;
                mask |= 1 << k;
            }
            mask
        }
        _ => return Err("malformed destination mask"),
    };
    Ok(Dst { reg, mask })
}

fn src(s: &str) -> Result<Src, &'static str> {
    let (reg, tail) = register(s)?;
    let swizzle = match tail.strip_prefix('.') {
        None if tail.is_empty() => [0, 1, 2, 3],
        Some(m) if m.len() == 4 => {
            let b = m.as_bytes();
            [component(b[0])?, component(b[1])?, component(b[2])?, component(b[3])?]
        }
        Some(m) if m.len() == 1 => [component(m.as_bytes()[0])?; 4],
        _ => return Err("malformed source swizzle"),
    };
    Ok(Src { reg, swizzle })
}

fn swizzle_bits(s: [u8; 4]) -> u32 {
    (s[0] as u32) | (s[1] as u32) << 2 | (s[2] as u32) << 4 | (s[3] as u32) << 6
}

/// The tokens of a destination operand: a masked temporary or output register.
fn dst_tokens(t: &Text<'_>, d: &Dst, out: &mut Vec<u32>) -> Result<(), &'static str> {
    let (kind, index) = match d.reg {
        Reg::Temp(n) if n < t.temps => (file::TEMP, n),
        Reg::Out(0) => (file::OUTPUT, 0),
        _ => return Err("destination register is not declared"),
    };
    out.extend_from_slice(&[FOUR | (d.mask << 4) | operand(kind, 1), index]);
    Ok(())
}

/// The tokens of a source operand. An immediate becomes a four-component literal holding
/// the swizzled values, so the program needs no immediate constant buffer. `select` reads
/// one component (an `if` or `discard` condition) instead of four.
fn src_tokens(t: &Text<'_>, s: &Src, select: bool, out: &mut Vec<u32>) -> Result<(), &'static str> {
    let mode = |out: &mut Vec<u32>, kind: u32, dims: u32| {
        out.push(if select {
            FOUR | MODE_SELECT1 | ((s.swizzle[0] as u32) << 4) | operand(kind, dims)
        } else {
            FOUR | MODE_SWIZZLE | (swizzle_bits(s.swizzle) << 4) | operand(kind, dims)
        });
    };
    match s.reg {
        Reg::Temp(n) if n < t.temps => {
            mode(out, file::TEMP, 1);
            out.push(n);
        }
        Reg::In(0) => {
            mode(out, file::INPUT, 1);
            out.push(0);
        }
        Reg::Const(n) if t.consts.is_some_and(|c| n < c) => {
            mode(out, file::INPUT, 1);
            out.push(1 + n);
        }
        Reg::Imm(n) if (n as usize) < t.imms.len() => {
            let v = t.imms[n as usize];
            if select {
                // A one-component immediate.
                out.extend_from_slice(&[1 | operand(file::IMMEDIATE32, 0), v[s.swizzle[0] as usize]]);
            } else {
                out.push(FOUR | operand(file::IMMEDIATE32, 0));
                for k in 0..4 {
                    out.push(v[s.swizzle[k] as usize]);
                }
            }
        }
        _ => return Err("source register is not declared"),
    }
    Ok(())
}

/// The SM4 instruction with the same per-component result as each TGSI arithmetic
/// instruction (the TGSI documentation's definitions; D3D10's instruction set).
fn arithmetic(name: &str) -> Option<(u32, usize)> {
    Some(match name {
        // Integer operations on 32-bit components: add (low 32 bits), bitwise and, or and xor,
        // shifts by the low five bits of the count, and comparisons that write 0xffffffff or 0.
        "UADD" => (op::IADD, 2),
        "AND" => (op::AND, 2),
        "OR" => (op::OR, 2),
        "XOR" => (op::XOR, 2),
        "SHL" => (op::ISHL, 2),
        "USHR" => (op::USHR, 2),
        "USLT" => (op::ULT, 2),
        "USEQ" => (op::IEQ, 2),
        "USNE" => (op::INE, 2),
        // Conversions: binary32 to signed and unsigned integers (truncating), and unsigned
        // integers to binary32.
        "F2I" => (op::FTOI, 1),
        "F2U" => (op::FTOU, 1),
        "U2F" => (op::UTOF, 1),
        // Binary32 arithmetic.
        "ADD" => (op::ADD, 2),
        "MUL" => (op::MUL, 2),
        "MOV" => (op::MOV, 1),
        _ => return None,
    })
}

fn instruction(w: &mut Vec<u32>, t: &Text<'_>, line: usize, s: &str) -> Result<(), Error> {
    let err = |what| Error { line, what };
    let (name, args) = match s.split_once(' ') {
        Some((n, a)) => (n, a.trim()),
        None => (s, ""),
    };
    let operands: Vec<&str> = if args.is_empty() { Vec::new() } else { args.split(',').map(str::trim).collect() };
    let mut toks: Vec<u32> = Vec::new();
    let opcode = match name {
        // UMUL: the low 32 bits of the unsigned product; SM4's umul writes the high half to its
        // first destination, discarded here into the null register.
        "UMUL" => {
            if operands.len() != 3 {
                return Err(err("UMUL takes a destination and two sources"));
            }
            toks.push(operand(file::NULL, 0));
            dst_tokens(t, &dst(operands[0]).map_err(err)?, &mut toks).map_err(err)?;
            for o in &operands[1..] {
                src_tokens(t, &src(o).map_err(err)?, false, &mut toks).map_err(err)?;
            }
            op::UMUL
        }
        // TXF: one texel at an integer address and level (address .xy, level .w here), no
        // filtering; SM4's ld reads the same texel of resource t#n.
        "TXF" => {
            if operands.len() != 4 || operands[3] != "2D" {
                return Err(err("TXF takes a destination, an address, a view and 2D"));
            }
            dst_tokens(t, &dst(operands[0]).map_err(err)?, &mut toks).map_err(err)?;
            src_tokens(t, &src(operands[1]).map_err(err)?, false, &mut toks).map_err(err)?;
            let view = src(operands[2]).map_err(err)?;
            let Reg::View(n) = view.reg else { return Err(err("TXF's third operand is not a view")) };
            if !t.views.contains(&n) {
                return Err(err("TXF names an undeclared view"));
            }
            toks.extend_from_slice(&[FOUR | MODE_SWIZZLE | (XYZW << 4) | operand(file::RESOURCE, 1), n]);
            op::LD
        }
        // UIF: the block runs when the condition's first component is nonzero.
        "UIF" => {
            if operands.len() != 1 {
                return Err(err("UIF takes one condition"));
            }
            src_tokens(t, &src(operands[0]).map_err(err)?, true, &mut toks).map_err(err)?;
            op::IF | TEST_NONZERO
        }
        "ELSE" | "ENDIF" | "BGNLOOP" | "ENDLOOP" | "BRK" if operands.is_empty() => match name {
            "ELSE" => op::ELSE,
            "ENDIF" => op::ENDIF,
            "BGNLOOP" => op::LOOP,
            "ENDLOOP" => op::ENDLOOP,
            _ => op::BREAK,
        },
        // KILL: the fragment is discarded unconditionally; SM4's discard with a nonzero
        // immediate condition.
        "KILL" if operands.is_empty() => {
            toks.extend_from_slice(&[1 | operand(file::IMMEDIATE32, 0), u32::MAX]);
            op::DISCARD | TEST_NONZERO
        }
        _ => {
            let (opcode, sources) = arithmetic(name).ok_or(err("unsupported instruction"))?;
            if operands.len() != sources + 1 {
                return Err(err("wrong operand count"));
            }
            dst_tokens(t, &dst(operands[0]).map_err(err)?, &mut toks).map_err(err)?;
            for o in &operands[1..] {
                src_tokens(t, &src(o).map_err(err)?, false, &mut toks).map_err(err)?;
            }
            opcode
        }
    };
    if toks.len() + 1 > 127 {
        return Err(err("instruction too long"));
    }
    inst(w, opcode, &toks);
    Ok(())
}
