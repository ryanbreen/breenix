//! A small interpreter of the emitted ISA, independent of the SSA evaluator.
use super::*;

#[derive(Clone, Copy)]
struct Reg {
    file: u8,
    index: usize,
    lanes: [usize; 4],
    mask: u8,
}
fn reg(s: &str) -> Reg {
    let open = s.find('[').unwrap();
    let close = s.find(']').unwrap();
    let file = match &s[..open] {
        "TEMP" => 0,
        "IMM" => 1,
        "CONST" => 2,
        "IN" => 3,
        "OUT" => 4,
        _ => panic!("{s}"),
    };
    let swizzle = s.get(close + 2..).unwrap_or("xyzw");
    let mut lanes = [0, 1, 2, 3];
    let mut mask = 0;
    for (i, c) in swizzle.bytes().enumerate() {
        let lane = b"xyzw".iter().position(|&v| v == c).unwrap();
        lanes[i] = lane;
        mask |= 1 << lane;
    }
    if swizzle.len() == 1 {
        let first = lanes[0];
        lanes.fill(first);
    }
    Reg {
        file,
        index: s[open + 1..close].parse().unwrap(),
        lanes,
        mask,
    }
}
struct Instruction {
    op: String,
    regs: Vec<Reg>,
    jump: usize,
}
struct Shader {
    instructions: Vec<Instruction>,
    immediates: Vec<[u32; 4]>,
}
impl Shader {
    fn parse(text: &str) -> Self {
        let mut instructions: Vec<Instruction> = Vec::new();
        let mut immediates = Vec::new();
        let mut stack = Vec::new();
        for line in text.trim_end_matches('\0').lines() {
            if line.starts_with("IMM[") {
                let begin = line.find('{').unwrap() + 1;
                let end = line.find('}').unwrap();
                let v: Vec<u32> = line[begin..end]
                    .split(", ")
                    .map(|s| s.parse().unwrap())
                    .collect();
                immediates.push(v.try_into().unwrap());
            } else if line == "FRAG" || line.starts_with("DCL") {
                continue;
            } else {
                let (op, args) = line.split_once(' ').unwrap_or((line, ""));
                let i = instructions.len();
                let mut jump = 0;
                if op == "UIF" || op == "BGNLOOP" {
                    stack.push(i);
                }
                if op == "ENDIF" || op == "ENDLOOP" {
                    let start = stack.pop().unwrap();
                    assert_eq!(
                        instructions[start].op,
                        if op == "ENDIF" { "UIF" } else { "BGNLOOP" }
                    );
                    instructions[start].jump = i + 1;
                    jump = start + 1;
                }
                instructions.push(Instruction {
                    op: op.to_string(),
                    regs: if args.is_empty() {
                        Vec::new()
                    } else {
                        args.split(", ").map(reg).collect()
                    },
                    jump,
                });
            }
        }
        assert!(stack.is_empty());
        Self {
            instructions,
            immediates,
        }
    }
    fn run(&self, routine: u32, word: u32, args: &[u32; 18]) -> u32 {
        let mut temp = vec![[0u32; 4]; 256];
        let constants: Vec<[u32; 4]> = args[..16]
            .chunks(4)
            .map(|a| a.try_into().unwrap())
            .chain([[word, routine, 0, 0]])
            .collect();
        let input = [[
            (args[16] as f32 + 0.5).to_bits(),
            (args[17] as f32 + 0.5).to_bits(),
            0,
            0,
        ]];
        let mut output = [[0u32; 4]];
        let mut pc = 0;
        let mut steps = 0;
        let mut loops = Vec::new();
        while pc < self.instructions.len() {
            steps += 1;
            assert!(steps <= 65536, "emitted program did not terminate");
            let ins = &self.instructions[pc];
            let read = |r: Reg| {
                let value = match r.file {
                    0 => temp[r.index],
                    1 => self.immediates[r.index],
                    2 => constants[r.index],
                    3 => input[r.index],
                    4 => output[r.index],
                    _ => unreachable!(),
                };
                r.lanes.map(|i| value[i])
            };
            match ins.op.as_str() {
                "END" => break,
                "UIF" => {
                    if read(ins.regs[0])[0] == 0 {
                        pc = ins.jump;
                        continue;
                    }
                }
                "ENDIF" => {}
                "BGNLOOP" => loops.push(ins.jump),
                "ENDLOOP" => {
                    pc = ins.jump;
                    continue;
                }
                "BRK" => {
                    pc = loops.pop().unwrap();
                    continue;
                }
                op => {
                    let d = ins.regs[0];
                    let a = read(ins.regs[1]);
                    let b = if ins.regs.len() > 2 {
                        read(ins.regs[2])
                    } else {
                        [0; 4]
                    };
                    let c = if ins.regs.len() > 3 {
                        read(ins.regs[3])
                    } else {
                        [0; 4]
                    };
                    let v = core::array::from_fn::<_, 4, _>(|i| match op {
                        "MOV" => a[i],
                        "UADD" => a[i].wrapping_add(b[i]),
                        "UMUL" => a[i].wrapping_mul(b[i]),
                        "AND" => a[i] & b[i],
                        "OR" => a[i] | b[i],
                        "XOR" => a[i] ^ b[i],
                        "SHL" => a[i].wrapping_shl(b[i]),
                        "USHR" => a[i].wrapping_shr(b[i]),
                        "USLT" => {
                            if a[i] < b[i] {
                                !0
                            } else {
                                0
                            }
                        }
                        "USEQ" => {
                            if a[i] == b[i] {
                                !0
                            } else {
                                0
                            }
                        }
                        "USNE" => {
                            if a[i] != b[i] {
                                !0
                            } else {
                                0
                            }
                        }
                        "UCMP" => {
                            if a[i] != 0 {
                                b[i]
                            } else {
                                c[i]
                            }
                        }
                        "F2I" => (f32::from_bits(a[i]) as i32) as u32,
                        "U2F" => (a[i] as f32).to_bits(),
                        "MUL" => (f32::from_bits(a[i]) * f32::from_bits(b[i])).to_bits(),
                        _ => panic!("unknown emitted opcode {op}"),
                    });
                    let destination = match d.file {
                        0 => &mut temp[d.index],
                        4 => &mut output[d.index],
                        _ => panic!("invalid destination"),
                    };
                    for i in 0..4 {
                        if d.mask & (1 << i) != 0 {
                            destination[i] = v[i];
                        }
                    }
                }
            }
            pc += 1;
        }
        let bytes: [u32; 4] = output[0].map(|v| (f32::from_bits(v) * 255.0).round() as u32);
        bytes[2] | bytes[1] << 8 | bytes[0] << 16 | bytes[3] << 24
    }
}

#[test]
fn final_integer_shader_matches_every_model_routine_and_word() {
    let (text, _) = generated();
    let shader = Shader::parse(&text);
    let mut seed = 0x9127a3d5u32;
    for routine in 0..25 {
        let (p, out) = program(routine);
        for _ in 0..24 {
            let args = core::array::from_fn(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                if i >= 16 {
                    seed & 16383
                } else {
                    seed
                }
            });
            let expected = p.eval(&args, &out);
            for (word, &want) in expected.iter().enumerate() {
                assert_eq!(
                    shader.run(routine, word as u32, &args),
                    want,
                    "routine={routine} word={word} args={args:?}"
                );
            }
        }
    }
}

#[test]
fn emitted_division_loop_matches_native_quotient_and_remainder() {
    let (text, _) = generated();
    let shader = Shader::parse(&text);
    let mask = (1u128 << 96) - 1;
    let pairs = [
        (0, 1),
        (mask, 1),
        (mask, mask),
        (mask, 1u128 << 95),
        (1u128 << 95, 3),
        (1u128 << 64, (1u128 << 32) + 1),
        (mask, (1u128 << 64) + 1),
    ];
    for (a, b) in pairs {
        let mut args = [0; 18];
        for i in 0..3 {
            args[i] = (a >> (32 * i)) as u32;
            args[i + 3] = (b >> (32 * i)) as u32;
        }
        for word in 0..6 {
            let want = if word < 3 { a / b } else { a % b };
            assert_eq!(
                shader.run(11, word, &args),
                (want >> (32 * (word % 3))) as u32,
                "{a:x}/{b:x} word={word}"
            );
        }
    }
}

#[test]
fn integer_rewrites_preserve_nan_shaped_words_and_aliasing() {
    let (p, out) = program(0);
    let (baseline, mut tokens, _) = tgsi(&p, &out);
    let instruction = "UADD TEMP[1].x, CONST[0].x, CONST[0].y";
    assert!(baseline.contains(instruction));
    let baseline = baseline.replace(
        instruction,
        "MOV TEMP[1].x, CONST[0].x\nUCMP TEMP[1].x, CONST[0].y, TEMP[1].x, CONST[0].z",
    );
    let expanded = expand_sources(&baseline);
    let rewritten = integer_words(&expanded, 4, &mut tokens);
    let (rewritten, _) = integer_constants(&rewritten, 7, &mut tokens);
    let shader = Shader::parse(&rewritten);
    for condition in [0, 1, u32::MAX] {
        for value in [0, 1, 0x7fc00000, 0xffffffff, 0xffffff69, 0x80000000] {
            let mut args = [0; 18];
            args[0] = value;
            args[1] = condition;
            args[2] = !value;
            assert_eq!(
                shader.run(0, 0, &args),
                if condition != 0 { value } else { !value }
            );
        }
    }
    // These fixed expectations specifically consume reconstructed NaN-shaped
    // constants, rather than only transporting dynamic words through a select.
    let (text, _) = generated();
    let shader = Shader::parse(&text);
    for (routine, inputs, expected) in [
        (12, [0x7fc00000, 0], 0x7fc00000),
        (12, [0x7f800000, 0xff800000], 0x7fc00000),
        (14, [0x7f800000, 0], 0x7fc00000),
        (16, [0x7fffffff, 0], 0x7fc00000),
    ] {
        let mut args = [0; 18];
        args[..2].copy_from_slice(&inputs);
        assert_eq!(shader.run(routine, 0, &args), expected);
    }
}

#[test]
fn rounded_rect_matches_independent_pixel_centre_reference() {
    let (p, out) = program(22);
    let (text, _) = generated();
    let shader = Shader::parse(&text);
    for (x0, y0, w, h, r) in [
        (0i32, 0i32, 8i32, 6i32, 2i32),
        (-3, -4, 7, 9, 0),
        (0, 0, 0, 9, 4),
        (0, 0, -1, 3, 1),
        (0, 0, 3, 0, 1),
        (0, 0, 9, 9, -1),
        (-2147483648, -2147483648, 2147483647, 2147483647, 2147483647),
        (0, 0, 2147483647, 2147483647, 1073741823),
    ] {
        for (x, y) in [
            (x0, y0),
            (x0 + 1, y0),
            (x0 + 2, y0 + 2),
            (x0 + w.saturating_sub(1), y0 + h.saturating_sub(1)),
            (x0 + w, y0),
            (x0 + w / 2, y0 + h / 2),
        ] {
            let mut args = [0u32; 16];
            for (dst, src) in args.iter_mut().zip([x, y, x0, y0, w, h, r]) {
                *dst = src as u32;
            }
            let r = i128::from(r.max(0).min(w.max(0) / 2).min(h.max(0) / 2));
            let px = i128::from(x) - i128::from(x0);
            let py = i128::from(y) - i128::from(y0);
            let dx = (2 * r - (2 * px + 1))
                .max(0)
                .max(2 * px + 1 - 2 * (i128::from(w) - r));
            let dy = (2 * r - (2 * py + 1))
                .max(0)
                .max(2 * py + 1 - 2 * (i128::from(h) - r));
            let want = w > 0
                && h > 0
                && px >= 0
                && py >= 0
                && px < i128::from(w)
                && py < i128::from(h)
                && dx * dx + dy * dy <= 4 * r * r;
            assert_eq!(p.eval(&args, &out)[0], u32::from(want), "{args:?}");
            let mut device = [0; 18];
            device[..16].copy_from_slice(&args);
            assert_eq!(
                shader.run(22, 0, &device),
                u32::from(want),
                "emitted rounded {args:?}"
            );
        }
    }
}

#[test]
fn triangle_matches_independent_edge_reference_including_ties_and_winding() {
    let (p, out) = program(23);
    let (text, _) = generated();
    let shader = Shader::parse(&text);
    let triangles = [
        [(0, 0), (4, 0), (0, 4)],
        [(0, 0), (0, 4), (4, 0)],
        [(0, 0), (2, 2), (4, 4)],
        [(0, 0), (0, 0), (4, 4)],
        [
            (i32::MIN, i32::MIN),
            (i32::MAX, i32::MIN),
            (i32::MIN, i32::MAX),
        ],
    ];
    for vertices in triangles {
        for (x, y) in [
            (-1, -1),
            (0, 0),
            (1, 1),
            (2, 1),
            (1, 2),
            (2, 2),
            (3, 0),
            (0, 3),
            (i32::MIN, i32::MIN),
            (i32::MAX, i32::MAX),
        ] {
            let cross = |a: (i32, i32), b: (i32, i32), c: (i128, i128)| {
                (i128::from(b.0) - i128::from(a.0)) * (c.1 - 2 * i128::from(a.1))
                    - (i128::from(b.1) - i128::from(a.1)) * (c.0 - 2 * i128::from(a.0))
            };
            let area = cross(
                vertices[0],
                vertices[1],
                (2 * i128::from(vertices[2].0), 2 * i128::from(vertices[2].1)),
            );
            let mut want = area != 0;
            for i in 0..3 {
                let (mut a, mut b) = (vertices[i], vertices[(i + 1) % 3]);
                if area < 0 {
                    core::mem::swap(&mut a, &mut b);
                }
                let edge = cross(a, b, (2 * i128::from(x) + 1, 2 * i128::from(y) + 1));
                let dx = i128::from(b.0) - i128::from(a.0);
                let dy = i128::from(b.1) - i128::from(a.1);
                want &= edge > 0 || (edge == 0 && (dy < 0 || (dy == 0 && dx > 0)));
            }
            let mut args = [0u32; 16];
            for (dst, src) in args.iter_mut().zip([
                x,
                y,
                vertices[0].0,
                vertices[0].1,
                vertices[1].0,
                vertices[1].1,
                vertices[2].0,
                vertices[2].1,
            ]) {
                *dst = src as u32;
            }
            assert_eq!(p.eval(&args, &out)[0], u32::from(want), "{args:?}");
            let mut device = [0; 18];
            device[..16].copy_from_slice(&args);
            assert_eq!(
                shader.run(23, 0, &device),
                u32::from(want),
                "emitted triangle {args:?}"
            );
        }
    }
}

#[test]
fn committed_shader_is_the_tested_generator_output() {
    let (text, _) = generated();
    assert_eq!(
        text.trim_end_matches('\0'),
        include_str!("../../kernel/src/gpu/virgl/profile/profile.tgsi")
    );
}
