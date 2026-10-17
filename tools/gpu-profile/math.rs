//! Breenix integer arithmetic model, authored from IEEE 754 and gpu.md section 1.5.
//! Values are SSA nodes so the same operations can later be encoded as TGSI.
use std::collections::HashMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Op {
    Input(usize),
    Lit(u32),
    Add,
    Mul,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Lt,
    Eq,
    Select,
}
#[derive(Clone, Copy)]
struct Node {
    op: Op,
    args: [usize; 3],
}
type V = usize;
#[derive(Default)]
struct Code {
    nodes: Vec<Node>,
    expressions: HashMap<(Op, [V; 3]), V>,
    expanding_division: bool,
    literals: HashMap<u32, V>,
    divisions: Vec<Division>,
}
struct Division {
    start: usize,
    end: usize,
    a: [V; 3],
    b: [V; 3],
    out: Vec<V>,
}
impl Code {
    fn node(&mut self, op: Op, mut args: [V; 3]) -> V {
        if !self.expanding_division {
            let literal = |v: V| match self.nodes[v].op {
                Op::Lit(k) => Some(k),
                _ => None,
            };
            if !matches!(op, Op::Input(_) | Op::Lit(_)) {
                let a = literal(args[0]);
                let b = literal(args[1]);
                if let (Some(a), Some(b)) = (a, b) {
                    let value = match op {
                        Op::Add => Some(a.wrapping_add(b)),
                        Op::Mul => Some(a.wrapping_mul(b)),
                        Op::And => Some(a & b),
                        Op::Or => Some(a | b),
                        Op::Xor => Some(a ^ b),
                        Op::Shl => Some(a.wrapping_shl(b)),
                        Op::Shr => Some(a.wrapping_shr(b)),
                        Op::Lt => Some(u32::from(a < b)),
                        Op::Eq => Some(u32::from(a == b)),
                        _ => None,
                    };
                    if let Some(value) = value {
                        return self.k(value);
                    }
                }
                match op {
                    Op::Add | Op::Or | Op::Xor if b == Some(0) => return args[0],
                    Op::Add | Op::Or | Op::Xor if a == Some(0) => return args[1],
                    Op::Mul | Op::And if a == Some(0) || b == Some(0) => return self.k(0),
                    Op::Mul if b == Some(1) => return args[0],
                    Op::Mul if a == Some(1) => return args[1],
                    Op::And if b == Some(u32::MAX) => return args[0],
                    Op::And if a == Some(u32::MAX) => return args[1],
                    Op::Shl | Op::Shr if b == Some(0) => return args[0],
                    Op::Select if args[1] == args[2] => return args[1],
                    Op::Select if a.is_some() => {
                        return if a == Some(0) { args[2] } else { args[1] }
                    }
                    _ => {}
                }
            }
            if matches!(op, Op::Add | Op::Mul | Op::And | Op::Or | Op::Xor | Op::Eq)
                && args[0] > args[1]
            {
                args.swap(0, 1);
            }
            if let Some(&v) = self.expressions.get(&(op, args)) {
                return v;
            }
        }
        let v = self.nodes.len();
        self.nodes.push(Node { op, args });
        if !self.expanding_division {
            self.expressions.insert((op, args), v);
        }
        v
    }
    fn input(&mut self, n: usize) -> V {
        self.node(Op::Input(n), [0; 3])
    }
    fn k(&mut self, x: u32) -> V {
        if let Some(&v) = self.literals.get(&x) {
            v
        } else {
            let v = self.node(Op::Lit(x), [0; 3]);
            self.literals.insert(x, v);
            v
        }
    }
    fn add(&mut self, a: V, b: V) -> V {
        self.node(Op::Add, [a, b, 0])
    }
    fn mul(&mut self, a: V, b: V) -> V {
        self.node(Op::Mul, [a, b, 0])
    }
    fn and(&mut self, a: V, b: V) -> V {
        self.node(Op::And, [a, b, 0])
    }
    fn or(&mut self, a: V, b: V) -> V {
        self.node(Op::Or, [a, b, 0])
    }
    fn xor(&mut self, a: V, b: V) -> V {
        self.node(Op::Xor, [a, b, 0])
    }
    fn shl(&mut self, a: V, b: V) -> V {
        self.node(Op::Shl, [a, b, 0])
    }
    fn shr(&mut self, a: V, b: V) -> V {
        self.node(Op::Shr, [a, b, 0])
    }
    fn lt(&mut self, a: V, b: V) -> V {
        self.node(Op::Lt, [a, b, 0])
    }
    fn eq(&mut self, a: V, b: V) -> V {
        self.node(Op::Eq, [a, b, 0])
    }
    fn sel(&mut self, c: V, a: V, b: V) -> V {
        self.node(Op::Select, [c, a, b])
    }
    fn not(&mut self, a: V) -> V {
        let k = self.k(!0);
        self.xor(a, k)
    }
    fn neg(&mut self, a: V) -> V {
        let n = self.not(a);
        let one = self.k(1);
        self.add(n, one)
    }
    fn sub(&mut self, a: V, b: V) -> V {
        let n = self.neg(b);
        self.add(a, n)
    }
    fn nz(&mut self, a: V) -> V {
        let z = self.k(0);
        self.lt(z, a)
    }
    fn slt(&mut self, a: V, b: V) -> V {
        let s = self.k(1 << 31);
        let a = self.xor(a, s);
        let b = self.xor(b, s);
        self.lt(a, b)
    }
    fn addk(&mut self, a: V, b: u32) -> V {
        let b = self.k(b);
        self.add(a, b)
    }
    fn andk(&mut self, a: V, b: u32) -> V {
        let b = self.k(b);
        self.and(a, b)
    }
    fn shrk(&mut self, a: V, b: u32) -> V {
        let b = self.k(b);
        self.shr(a, b)
    }
    fn shlk(&mut self, a: V, b: u32) -> V {
        let b = self.k(b);
        self.shl(a, b)
    }
    #[cfg(test)]
    fn eval(&self, args: &[u32], outputs: &[V]) -> Vec<u32> {
        let mut vs = Vec::<u32>::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let value = match n.op {
                Op::Input(n) => args[n],
                Op::Lit(k) => k,
                _ => {
                    let a = vs[n.args[0]];
                    let b = vs[n.args[1]];
                    match n.op {
                        Op::Add => a.wrapping_add(b),
                        Op::Mul => a.wrapping_mul(b),
                        Op::And => a & b,
                        Op::Or => a | b,
                        Op::Xor => a ^ b,
                        Op::Shl => a.wrapping_shl(b),
                        Op::Shr => a.wrapping_shr(b),
                        Op::Lt => u32::from(a < b),
                        Op::Eq => u32::from(a == b),
                        Op::Select => {
                            if a != 0 {
                                b
                            } else {
                                vs[n.args[2]]
                            }
                        }
                        Op::Input(_) | Op::Lit(_) => unreachable!(),
                    }
                }
            };
            vs.push(value);
        }
        outputs.iter().map(|&v| vs[v]).collect()
    }
    /// Exact 32x32 product assembled from four 16x16 partial products.
    fn product(&mut self, a: V, b: V) -> [V; 2] {
        let al = self.andk(a, 65535);
        let ah = self.shrk(a, 16);
        let bl = self.andk(b, 65535);
        let bh = self.shrk(b, 16);
        let ll = self.mul(al, bl);
        let hl = self.mul(ah, bl);
        let lh = self.mul(al, bh);
        let hh = self.mul(ah, bh);
        let llh = self.shrk(ll, 16);
        let hll = self.andk(hl, 65535);
        let lhl = self.andk(lh, 65535);
        let mid = self.add(llh, hll);
        let mid = self.add(mid, lhl);
        let lo = self.andk(ll, 65535);
        let ml = self.shlk(mid, 16);
        let lo = self.or(lo, ml);
        let hlh = self.shrk(hl, 16);
        let lhh = self.shrk(lh, 16);
        let mh = self.shrk(mid, 16);
        let hi = self.add(hh, hlh);
        let hi = self.add(hi, lhh);
        let hi = self.add(hi, mh);
        [lo, hi]
    }
    fn addwide(&mut self, a: &[V], b: &[V]) -> Vec<V> {
        let mut carry = self.k(0);
        let mut out = Vec::new();
        for (&a, &b) in a.iter().zip(b) {
            let s = self.add(a, b);
            let c1 = self.lt(s, a);
            let t = self.add(s, carry);
            let c2 = self.lt(t, s);
            carry = self.or(c1, c2);
            out.push(t);
        }
        out
    }
    fn subwide(&mut self, a: &[V], b: &[V]) -> Vec<V> {
        let mut borrow = self.k(0);
        let mut out = Vec::new();
        for (&a, &b) in a.iter().zip(b) {
            let s = self.sub(a, b);
            let c1 = self.lt(a, b);
            let t = self.sub(s, borrow);
            let c2 = self.lt(s, borrow);
            borrow = self.or(c1, c2);
            out.push(t);
        }
        out
    }
    fn ltwide(&mut self, a: &[V], b: &[V]) -> V {
        let mut lt = self.k(0);
        for (&a, &b) in a.iter().zip(b) {
            let less = self.lt(a, b);
            let eq = self.eq(a, b);
            lt = self.sel(eq, lt, less);
        }
        lt
    }
    fn div96(&mut self, a: [V; 3], b: [V; 3]) -> Vec<V> {
        let start = self.nodes.len();
        self.expanding_division = true;
        let z = self.k(0);
        let one = self.k(1);
        let mut r = vec![z; 3];
        let mut q = vec![z; 3];
        for bit in (0..96).rev() {
            let overflow = self.shrk(r[2], 31);
            let incoming = self.shrk(a[bit / 32], (bit % 32) as u32);
            let incoming = self.and(incoming, one);
            let mut carry = incoming;
            for v in &mut r {
                let next = self.shrk(*v, 31);
                let shifted = self.shlk(*v, 1);
                *v = self.or(shifted, carry);
                carry = next;
            }
            let lt = self.ltwide(&r, &b);
            let ge = self.eq(lt, z);
            let take = self.or(overflow, ge);
            let sub = self.subwide(&r, &b);
            for i in 0..3 {
                r[i] = self.sel(take, sub[i], r[i]);
            }
            let mask = self.shlk(take, (bit % 32) as u32);
            q[bit / 32] = self.or(q[bit / 32], mask);
        }
        q.extend(r);
        self.expanding_division = false;
        self.divisions.push(Division {
            start,
            end: self.nodes.len(),
            a,
            b,
            out: q.clone(),
        });
        q
    }
    /// Shift right with a sticky low bit, including shifts of 32 or more.
    fn sticky_shift(&mut self, a: V, n: V) -> V {
        let k31 = self.k(31);
        let large = self.lt(k31, n);
        let s = self.sel(large, k31, n);
        let one = self.k(1);
        let mask = self.shl(one, s);
        let mask = self.addk(mask, !0);
        let low = self.and(a, mask);
        let low = self.nz(low);
        let hi = self.shr(a, s);
        let joined = self.or(hi, low);
        let any = self.nz(a);
        self.sel(large, any, joined)
    }
    fn parts(&mut self, a: V) -> (V, V, V) {
        let sign = self.andk(a, 1 << 31);
        let e = self.shrk(a, 23);
        let e = self.andk(e, 255);
        let f = self.andk(a, 0x7fffff);
        let nonzero = self.nz(e);
        let hidden = self.k(1 << 23);
        let hidden = {
            let z = self.k(0);
            self.sel(nonzero, hidden, z)
        };
        let m = self.or(f, hidden);
        let one = self.k(1);
        let e = self.sel(nonzero, e, one);
        (sign, e, m)
    }
    fn round(&mut self, sign: V, mut e: V, mut m: V) -> V {
        let one = self.k(1);
        let low = self.slt(e, one);
        let n = self.sub(one, e);
        let j = self.sticky_shift(m, n);
        m = self.sel(low, j, m);
        e = self.sel(low, one, e);
        let rem = self.andk(m, 7);
        let k4 = self.k(4);
        let gt = self.lt(k4, rem);
        let eq = self.eq(rem, k4);
        let mant = self.shrk(m, 3);
        let odd = self.andk(mant, 1);
        let tie = self.and(eq, odd);
        let inc = self.or(gt, tie);
        let mant = self.add(mant, inc);
        let over = self.shrk(mant, 24);
        let shr = self.shrk(mant, 1);
        let mant = self.sel(over, shr, mant);
        e = self.add(e, over);
        let hidden = self.k(1 << 23);
        let sub = self.lt(mant, hidden);
        let zero = self.k(0);
        let e = self.sel(sub, zero, e);
        let exp = self.shlk(e, 23);
        let fraction = self.andk(mant, 0x7fffff);
        let result = self.or(exp, fraction);
        let result = self.or(result, sign);
        let k255 = self.k(255);
        let overflow = self.slt(e, k255);
        let inf = self.k(0x7f800000);
        let inf = self.or(sign, inf);
        self.sel(overflow, result, inf)
    }
    fn float_add(&mut self, a: V, b: V) -> V {
        let aa = self.andk(a, 0x7fffffff);
        let ab = self.andk(b, 0x7fffffff);
        let swap = self.lt(aa, ab);
        let hi = self.sel(swap, b, a);
        let lo = self.sel(swap, a, b);
        let (sign, mut e, mh) = self.parts(hi);
        let (sl, el, ml) = self.parts(lo);
        let mh = self.shlk(mh, 3);
        let ml = self.shlk(ml, 3);
        let diff = self.sub(e, el);
        let ml = self.sticky_shift(ml, diff);
        let same = self.eq(sign, sl);
        let plus = self.add(mh, ml);
        let minus = self.sub(mh, ml);
        let mut m = self.sel(same, plus, minus);
        let over = self.shrk(m, 27);
        let one = self.k(1);
        let shift = self.sticky_shift(m, one);
        m = self.sel(over, shift, m);
        e = self.add(e, over);
        for _ in 0..26 {
            let k = self.k(1 << 26);
            let small = self.lt(m, k);
            let enz = self.lt(one, e);
            let shift = self.and(small, enz);
            let sh = self.shlk(m, 1);
            m = self.sel(shift, sh, m);
            let dec = self.addk(e, !0);
            e = self.sel(shift, dec, e);
        }
        let nonzero = self.nz(m);
        let keep = self.or(nonzero, same);
        let zero = self.k(0);
        let sign = self.sel(keep, sign, zero);
        let result = self.round(sign, e, m);
        let inf = self.k(0x7f800000);
        let hiabs = self.andk(hi, 0x7fffffff);
        let highinf = self.eq(hiabs, inf);
        let ainf = self.eq(aa, inf);
        let binf = self.eq(ab, inf);
        let both = self.and(ainf, binf);
        let opposite = self.eq(same, zero);
        let invalid = self.and(both, opposite);
        let anan = self.lt(inf, aa);
        let bnan = self.lt(inf, ab);
        let nan = self.or(anan, bnan);
        let nan = self.or(nan, invalid);
        let canon = self.k(0x7fc00000);
        let result = self.sel(highinf, hi, result);
        self.sel(nan, canon, result)
    }
    fn float_mul(&mut self, a: V, b: V) -> V {
        let (sa, mut ea, mut ma) = self.parts(a);
        let (sb, mut eb, mut mb) = self.parts(b);
        let sign = self.xor(sa, sb);
        for _ in 0..23 {
            let hidden = self.k(1 << 23);
            let c = self.lt(ma, hidden);
            let sh = self.shlk(ma, 1);
            ma = self.sel(c, sh, ma);
            let dec = self.addk(ea, !0);
            ea = self.sel(c, dec, ea);
            let c = self.lt(mb, hidden);
            let sh = self.shlk(mb, 1);
            mb = self.sel(c, sh, mb);
            let dec = self.addk(eb, !0);
            eb = self.sel(c, dec, eb);
        }
        let [lo, hi] = self.product(ma, mb);
        let high = self.shrk(hi, 15);
        let h = self.shlk(hi, 12);
        let l = self.shrk(lo, 20);
        let m = self.or(h, l);
        let low = self.andk(lo, 0xfffff);
        let sticky = self.nz(low);
        let m = self.or(m, sticky);
        let one = self.k(1);
        let shift = self.sticky_shift(m, one);
        let m = self.sel(high, shift, m);
        let e = self.add(ea, eb);
        let e = self.addk(e, 0u32.wrapping_sub(127));
        let e = self.add(e, high);
        let mut result = self.round(sign, e, m);
        let aa = self.andk(a, 0x7fffffff);
        let ab = self.andk(b, 0x7fffffff);
        let z = self.k(0);
        let az = self.eq(aa, z);
        let bz = self.eq(ab, z);
        let zeros = self.or(az, bz);
        result = self.sel(zeros, sign, result);
        let inf = self.k(0x7f800000);
        let ai = self.eq(aa, inf);
        let bi = self.eq(ab, inf);
        let infinities = self.or(ai, bi);
        let signedinf = self.or(sign, inf);
        result = self.sel(infinities, signedinf, result);
        let invalid = self.and(infinities, zeros);
        let anan = self.lt(inf, aa);
        let bnan = self.lt(inf, ab);
        let nan = self.or(anan, bnan);
        let nan = self.or(nan, invalid);
        let canon = self.k(0x7fc00000);
        self.sel(nan, canon, result)
    }
}

fn program(routine: u32) -> (Code, Vec<V>) {
    let mut c = Code::default();
    let a: Vec<V> = (0..16).map(|i| c.input(i)).collect();
    let outputs = match routine {
        0 => vec![c.add(a[0], a[1])],
        1 => vec![c.sub(a[0], a[1])],
        2 => vec![c.mul(a[0], a[1])],
        3 => vec![c.shl(a[0], a[1])],
        4 => vec![c.shr(a[0], a[1])],
        5 => {
            let right = c.shr(a[0], a[1]);
            let sign = c.shrk(a[0], 31);
            let all = c.neg(sign);
            let n = c.k(32);
            let n = c.sub(n, a[1]);
            let fill = c.shl(all, n);
            let z = c.k(0);
            let iszero = c.eq(a[1], z);
            let fill = c.sel(iszero, z, fill);
            vec![c.or(right, fill)]
        }
        8 => {
            let n = c.addk(a[0], 128);
            let hi = c.shrk(n, 8);
            let n = c.add(n, hi);
            vec![c.shrk(n, 8)]
        }
        15 => vec![c.float_trunc(a[0])],
        16 => vec![c.float_ceil(a[0])],
        17 => vec![c.float_less(a[0], a[1])],
        18 => vec![c.input(16)],
        19 => vec![c.input(17)],
        20 => vec![c.circle(&a, false)],
        21 => vec![c.circle(&a, true)],
        22 => vec![c.rounded(&a)],
        23 => vec![c.triangle(&a)],
        24 => vec![c.line(&a)],
        6 => vec![c.slt(a[0], a[1])],
        7 => vec![c.lt(a[0], a[1])],
        9 => c.product(a[0], a[1]).to_vec(),
        10 => {
            let [lo, hi] = c.product(a[0], a[2]);
            let [hl, hh] = c.product(a[1], a[2]);
            let mid = c.add(hi, hl);
            let carry = c.lt(mid, hi);
            let top = c.add(hh, carry);
            vec![lo, mid, top]
        }
        11 => c.div96([a[0], a[1], a[2]], [a[3], a[4], a[5]]),
        12 => vec![c.float_add(a[0], a[1])],
        13 => {
            let sign = c.k(1 << 31);
            let b = c.xor(a[1], sign);
            vec![c.float_add(a[0], b)]
        }
        14 => vec![c.float_mul(a[0], a[1])],
        _ => panic!("routine not implemented: {routine}"),
    };
    (c, outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn native(bits: u32) -> u32 {
        if f32::from_bits(bits).is_nan() {
            0x7fc00000
        } else {
            bits
        }
    }
    #[test]
    fn binary32_model_matches_native_arithmetic() {
        let programs = [program(12), program(13), program(14)];
        let special = [
            0, 0x80000000, 1, 0x80000001, 0x7fffff, 0x800000, 0x3f000000, 0x3f800000, 0x3f800001,
            0x33800000, 0x7f7fffff, 0x7f800000, 0xff800000, 0x7fc00000,
        ];
        let mut seed = 0xA53C971Bu32;
        let check = |a: u32, b: u32| {
            let x = f32::from_bits(a);
            let y = f32::from_bits(b);
            let want = [(x + y).to_bits(), (x - y).to_bits(), (x * y).to_bits()];
            for (i, (p, os)) in programs.iter().enumerate() {
                let mut args = [0; 16];
                args[0] = a;
                args[1] = b;
                assert_eq!(
                    p.eval(&args, os)[0],
                    native(want[i]),
                    "op {} a={a:08x} b={b:08x}",
                    12 + i
                );
            }
        };
        for a in special {
            for b in special {
                check(a, b);
            }
        }
        for _ in 0..2000 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let a = seed;
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            check(a, seed);
        }
    }
    #[test]
    fn three_word_product_matches_native_low_96_bits() {
        let mut p = Code::default();
        let a: Vec<_> = (0..6).map(|i| p.input(i)).collect();
        let out = p.product96(&a[..3], &a[3..]);
        let mut seed = 0x1963A517u32;
        for _ in 0..1000 {
            let mut args = [0; 6];
            for v in &mut args {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                *v = seed;
            }
            let left = args[0] as u128 | (args[1] as u128) << 32 | (args[2] as u128) << 64;
            let right = args[3] as u128 | (args[4] as u128) << 32 | (args[5] as u128) << 64;
            let product = left.wrapping_mul(right);
            for (i, word) in p.eval(&args, &out).into_iter().enumerate() {
                assert_eq!(word, (product >> (32 * i)) as u32);
            }
        }
    }
    #[test]
    fn wide_model_matches_native_products_and_division() {
        for routine in [9, 10, 11] {
            let (p, os) = program(routine);
            let mut seed = 0x9137ab55u32;
            for _ in 0..100 {
                let mut a = [0; 16];
                for v in &mut a[..6] {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    *v = seed;
                }
                let want = match routine {
                    9 => (a[0] as u128) * (a[1] as u128),
                    10 => ((a[0] as u128) | ((a[1] as u128) << 32)) * (a[2] as u128),
                    _ => {
                        let n = a[0] as u128 | (a[1] as u128) << 32 | (a[2] as u128) << 64;
                        let d = a[3] as u128 | (a[4] as u128) << 32 | (a[5] as u128) << 64;
                        let got = p.eval(&a, &os);
                        let q = got[0] as u128 | (got[1] as u128) << 32 | (got[2] as u128) << 64;
                        let r = got[3] as u128 | (got[4] as u128) << 32 | (got[5] as u128) << 64;
                        assert_eq!((q, r), (n / d, n % d));
                        continue;
                    }
                };
                let got = p.eval(&a, &os);
                for (i, &v) in got.iter().enumerate() {
                    assert_eq!(v, (want >> (i * 32)) as u32);
                }
            }
        }
    }
}

impl Code {
    fn float_trunc(&mut self, a: V) -> V {
        let (sign, e, m) = self.parts(a);
        let k150 = self.k(150);
        let left = self.lt(k150, e);
        let l = self.sub(e, k150);
        let r = self.sub(k150, e);
        let magl = self.shl(m, l);
        let magr = self.shr(m, r);
        let mag = self.sel(left, magl, magr);
        let k127 = self.k(127);
        let small = self.lt(e, k127);
        let zero = self.k(0);
        let mag = self.sel(small, zero, mag);
        let neg = self.neg(mag);
        self.sel(sign, neg, mag)
    }
    fn float_ceil(&mut self, a: V) -> V {
        let abs = self.andk(a, 0x7fffffff);
        let sign = self.andk(a, 1 << 31);
        let e = self.shrk(abs, 23);
        let k150 = self.k(150);
        let large = self.lt(e, k150);
        let n = self.sub(k150, e);
        let one = self.k(1);
        let bit = self.shl(one, n);
        let mask = self.addk(bit, !0);
        let f = self.and(a, mask);
        let has = self.nz(f);
        let inv = self.not(mask);
        let trunc = self.and(a, inv);
        let rounded = self.add(trunc, bit);
        let positive = self.sel(has, rounded, trunc);
        let general = self.sel(sign, trunc, positive);
        let result = self.sel(large, general, a);
        let k127 = self.k(127);
        let small = self.lt(e, k127);
        let zero = self.k(0);
        let onef = self.k(0x3f800000);
        let nz = self.nz(abs);
        let positive = self.sel(nz, onef, zero);
        let smallvalue = self.sel(sign, sign, positive);
        let result = self.sel(small, smallvalue, result);
        let inf = self.k(0x7f800000);
        let nan = self.lt(inf, abs);
        let canon = self.k(0x7fc00000);
        self.sel(nan, canon, result)
    }
    fn float_less(&mut self, a: V, b: V) -> V {
        let aa = self.andk(a, 0x7fffffff);
        let ab = self.andk(b, 0x7fffffff);
        let sa = self.shrk(a, 31);
        let sb = self.shrk(b, 31);
        let same = self.eq(sa, sb);
        let positive = self.lt(aa, ab);
        let negative = self.lt(ab, aa);
        let cmp = self.sel(sa, negative, positive);
        let cmp = self.sel(same, cmp, sa);
        let both = self.or(aa, ab);
        let nz = self.nz(both);
        let cmp = self.and(cmp, nz);
        let inf = self.k(0x7f800000);
        let an = self.lt(inf, aa);
        let bn = self.lt(inf, ab);
        let nan = self.or(an, bn);
        let zero = self.k(0);
        self.sel(nan, zero, cmp)
    }
    fn absdiff(&mut self, a: V, b: V) -> V {
        let less = self.slt(a, b);
        let ab = self.sub(a, b);
        let ba = self.sub(b, a);
        self.sel(less, ba, ab)
    }
    fn wide(&mut self, a: V) -> Vec<V> {
        let sign = self.shrk(a, 31);
        let sign = self.neg(sign);
        vec![a, sign, sign]
    }
    fn product96(&mut self, a: &[V], b: &[V]) -> Vec<V> {
        let [lo, mut mid] = self.product(a[0], b[0]);
        let mut high = self.k(0);
        for (x, y) in [(a[0], b[1]), (a[1], b[0])] {
            let [part, top] = self.product(x, y);
            let sum = self.add(mid, part);
            let carry = self.lt(sum, mid);
            high = self.add(high, top);
            high = self.add(high, carry);
            mid = sum;
        }
        for (x, y) in [(a[0], b[2]), (a[1], b[1]), (a[2], b[0])] {
            let low = self.mul(x, y);
            high = self.add(high, low);
        }
        vec![lo, mid, high]
    }
    fn bool_and(&mut self, conditions: &[V]) -> V {
        let mut out = self.k(1);
        for &v in conditions {
            out = self.and(out, v);
        }
        out
    }
    fn circle(&mut self, a: &[V], outline: bool) -> V {
        let x = self.absdiff(a[0], a[2]);
        let y = self.absdiff(a[1], a[3]);
        let r = a[4];
        let zero = self.k(0);
        let negative = self.slt(r, zero);
        let valid = self.eq(negative, zero);
        let xr = self.lt(r, x);
        let yr = self.lt(r, y);
        let outside = self.or(xr, yr);
        let inside = self.eq(outside, zero);
        let xx = self.product(x, x);
        let yy = self.product(y, y);
        let sum = self.addwide(&xx, &yy);
        let rr = self.product(r, r);
        let membership = if !outline {
            let less = self.ltwide(&rr, &sum);
            self.eq(less, zero)
        } else {
            let swap = self.lt(x, y);
            let mx = self.sel(swap, y, x);
            let my = self.sel(swap, x, y);
            let xx = self.product(mx, mx);
            let yy = self.product(my, my);
            let sum = self.addwide(&xx, &yy);
            let mut vx = vec![mx, zero];
            let f = self.subwide(&sum, &vx);
            let less = self.ltwide(&rr, &f);
            let lower = self.eq(less, zero);
            let xp = self.addk(mx, 1);
            let xp2 = self.product(xp, xp);
            let sum = self.addwide(&xp2, &yy);
            vx[0] = xp;
            let next = self.subwide(&sum, &vx);
            let upper = self.ltwide(&rr, &next);
            self.and(lower, upper)
        };
        let rzero = self.eq(r, zero);
        let xy = self.or(x, y);
        let centre = self.eq(xy, zero);
        let membership = self.sel(rzero, centre, membership);
        self.bool_and(&[valid, inside, membership])
    }
    fn rounded(&mut self, a: &[V]) -> V {
        let zero = self.k(0);
        let x = self.sub(a[0], a[2]);
        let y = self.sub(a[1], a[3]);
        let width = a[4];
        let height = a[5];
        let validw = self.slt(zero, width);
        let validh = self.slt(zero, height);
        let inx = self.lt(x, width);
        let iny = self.lt(y, height);
        let beforex = self.slt(a[0], a[2]);
        let beforey = self.slt(a[1], a[3]);
        let afterx = self.eq(beforex, zero);
        let aftery = self.eq(beforey, zero);
        let inx = self.and(inx, afterx);
        let iny = self.and(iny, aftery);
        let rneg = self.slt(a[6], zero);
        let mut r = self.sel(rneg, zero, a[6]);
        let wh = self.shrk(width, 1);
        let hh = self.shrk(height, 1);
        let smaller = self.lt(wh, r);
        r = self.sel(smaller, wh, r);
        let smaller = self.lt(hh, r);
        r = self.sel(smaller, hh, r);
        let mut distances = Vec::new();
        for (p, size) in [(x, width), (y, height)] {
            let left = self.lt(p, r);
            let sr = self.sub(size, r);
            let right = self.lt(p, sr);
            let right = self.eq(right, zero);
            let dleft = self.sub(r, p);
            let dleft = self.shlk(dleft, 1);
            let dleft = self.addk(dleft, !0);
            let dright = self.sub(p, sr);
            let dright = self.shlk(dright, 1);
            let dright = self.addk(dright, 1);
            let d = self.sel(right, dright, zero);
            distances.push(self.sel(left, dleft, d));
        }
        let xx = self.product(distances[0], distances[0]);
        let yy = self.product(distances[1], distances[1]);
        let xx = [xx[0], xx[1], zero];
        let yy = [yy[0], yy[1], zero];
        let sum = self.addwide(&xx, &yy);
        let twice = self.shlk(r, 1);
        let rr = self.product(twice, twice);
        let rr = [rr[0], rr[1], zero];
        let outside = self.ltwide(&rr, &sum);
        let member = self.eq(outside, zero);
        self.bool_and(&[validw, validh, inx, iny, member])
    }
    fn cross(&mut self, ax: &[V], ay: &[V], bx: &[V], by: &[V]) -> Vec<V> {
        let l = self.product96(ax, by);
        let r = self.product96(ay, bx);
        self.subwide(&l, &r)
    }
    fn triangle(&mut self, a: &[V]) -> V {
        let z = self.k(0);
        let one = self.k(1);
        let mut px = self.wide(a[0]);
        let mut py = self.wide(a[1]);
        px = self.addwide(&px, &px.clone());
        py = self.addwide(&py, &py.clone());
        px = self.addwide(&px, &[one, z, z]);
        py = self.addwide(&py, &[one, z, z]);
        let x0 = self.wide(a[2]);
        let y0 = self.wide(a[3]);
        let x1 = self.wide(a[4]);
        let y1 = self.wide(a[5]);
        let x2 = self.wide(a[6]);
        let y2 = self.wide(a[7]);
        let dx = self.subwide(&x1, &x0);
        let dy = self.subwide(&y1, &y0);
        let ex = self.subwide(&x2, &x0);
        let ey = self.subwide(&y2, &y0);
        let area = self.cross(&dx, &dy, &ex, &ey);
        let negative = self.shrk(area[2], 31);
        let area01 = self.or(area[0], area[1]);
        let areaall = self.or(area01, area[2]);
        let valid = self.nz(areaall);
        let mut result = valid;
        for (ax, ay, bx, by) in [
            (&x0, &y0, &x1, &y1),
            (&x1, &y1, &x2, &y2),
            (&x2, &y2, &x0, &y0),
        ] {
            let mut dx = self.subwide(bx, ax);
            let mut dy = self.subwide(by, ay);
            let ndx = self.subwide(&[z, z, z], &dx);
            let ndy = self.subwide(&[z, z, z], &dy);
            for i in 0..3 {
                dx[i] = self.sel(negative, ndx[i], dx[i]);
                dy[i] = self.sel(negative, ndy[i], dy[i]);
            }
            let ax2 = self.addwide(ax, ax);
            let ay2 = self.addwide(ay, ay);
            let ex = self.subwide(&px, &ax2);
            let ey = self.subwide(&py, &ay2);
            let edge = self.cross(&dx, &dy, &ex, &ey);
            let neg = self.shrk(edge[2], 31);
            let nonneg = self.eq(neg, z);
            let dyneg = self.shrk(dy[2], 31);
            let dy01 = self.or(dy[0], dy[1]);
            let dyall = self.or(dy01, dy[2]);
            let dyeq = self.eq(dyall, z);
            let dxneg = self.shrk(dx[2], 31);
            let dxpos = self.eq(dxneg, z);
            let dx01 = self.or(dx[0], dx[1]);
            let dxall = self.or(dx01, dx[2]);
            let dxnz = self.nz(dxall);
            let dxpos = self.and(dxpos, dxnz);
            let horizontal = self.and(dyeq, dxpos);
            let top = self.or(dyneg, horizontal);
            let e01 = self.or(edge[0], edge[1]);
            let eall = self.or(e01, edge[2]);
            let enz = self.nz(eall);
            let accept = self.or(enz, top);
            let accept = self.and(accept, nonneg);
            result = self.and(result, accept);
        }
        result
    }
    fn line(&mut self, a: &[V]) -> V {
        let dx = self.absdiff(a[4], a[2]);
        let dy = self.absdiff(a[5], a[3]);
        let x = self.absdiff(a[0], a[2]);
        let y = self.absdiff(a[1], a[3]);
        let xmajor = self.lt(dy, dx);
        let major = self.sel(xmajor, dx, dy);
        let minor = self.sel(xmajor, dy, dx);
        let k = self.sel(xmajor, x, y);
        let v = self.sel(xmajor, y, x);
        let mut bounds = Vec::new();
        let zero = self.k(0);
        for (p, start, end, dist, span) in [(a[0], a[2], a[4], x, dx), (a[1], a[3], a[5], y, dy)] {
            let past = self.lt(span, dist);
            let inrange = self.eq(past, zero);
            let direction = self.slt(start, end);
            let ahead = self.slt(p, start);
            let behind = self.slt(start, p);
            let wrong = self.sel(direction, ahead, behind);
            let dir = self.eq(wrong, zero);
            bounds.push(self.and(inrange, dir));
        }
        let [lo, hi] = self.product(k, minor);
        let product = [lo, hi, zero];
        let twice = self.addwide(&product, &product);
        let numer = self.addwide(&twice, &[major, zero, zero]);
        let denom = self.addwide(&[major, zero, zero], &[major, zero, zero]);
        let q = self.div96(
            [numer[0], numer[1], numer[2]],
            [denom[0], denom[1], denom[2]],
        );
        let eq = self.eq(q[0], v);
        let qh = self.or(q[1], q[2]);
        let eqh = self.eq(qh, zero);
        let eq = self.and(eq, eqh);
        let point = self.eq(major, zero);
        let one = self.k(1);
        let matched = self.sel(point, one, eq);
        bounds.push(matched);
        self.bool_and(&bounds)
    }
}

#[cfg(test)]
mod predicate_tests {
    use super::*;
    fn model(r: u32, a: &[u32]) -> u32 {
        let (p, o) = program(r);
        let mut args = [0; 16];
        args[..a.len()].copy_from_slice(a);
        p.eval(&args, &o)[0]
    }
    #[test]
    fn circle_predicates_match_the_contract_walk() {
        for radius in 0i32..12 {
            let (fill, fo) = program(20);
            let (outline, oo) = program(21);
            let mut points = std::collections::HashSet::new();
            let (mut x, mut y, mut d) = (radius, 0, 1 - radius);
            while x >= y {
                for (x, y) in [
                    (x, y),
                    (y, x),
                    (-x, y),
                    (-y, x),
                    (x, -y),
                    (y, -x),
                    (-x, -y),
                    (-y, -x),
                ] {
                    points.insert((x, y));
                }
                y += 1;
                if d <= 0 {
                    d += 2 * y + 1;
                } else {
                    x -= 1;
                    d += 2 * (y - x) + 1;
                }
            }
            for y in -radius - 1..=radius + 1 {
                for x in -radius - 1..=radius + 1 {
                    let mut a = [0; 16];
                    a[0] = x as u32;
                    a[1] = y as u32;
                    a[4] = radius as u32;
                    assert_eq!(
                        fill.eval(&a, &fo)[0],
                        u32::from(x * x + y * y <= radius * radius)
                    );
                    assert_eq!(
                        outline.eval(&a, &oo)[0],
                        u32::from(points.contains(&(x, y))),
                        "r={radius} x={x} y={y}"
                    );
                }
            }
        }
    }
    #[test]
    fn line_predicate_matches_the_unclipped_walk() {
        let (p, o) = program(24);
        for endy in -5i32..=5 {
            for endx in -5i32..=5 {
                let mut points = std::collections::HashSet::new();
                let (mut x, mut y) = (0i32, 0i32);
                let (dx, dy) = (endx.abs(), -endy.abs());
                let (sx, sy) = (if endx > 0 { 1 } else { -1 }, if endy > 0 { 1 } else { -1 });
                let mut err = dx + dy;
                loop {
                    points.insert((x, y));
                    if (x, y) == (endx, endy) {
                        break;
                    }
                    let e2 = 2 * err;
                    if e2 >= dy {
                        err += dy;
                        x += sx;
                    }
                    if e2 <= dx {
                        err += dx;
                        y += sy;
                    }
                }
                for y in -6i32..=6 {
                    for x in -6i32..=6 {
                        let mut a = [0; 16];
                        a[0] = x as u32;
                        a[1] = y as u32;
                        a[4] = endx as u32;
                        a[5] = endy as u32;
                        assert_eq!(
                            p.eval(&a, &o)[0],
                            u32::from(points.contains(&(x, y))),
                            "end=({endx},{endy}) p=({x},{y})"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn large_shape_literals() {
        assert_eq!(model(20, &[0x7fffffff, 1, 0, 0, 0x7fffffff]), 0);
        assert_eq!(model(21, &[0x7fffffff, 1, 0, 0, 0x7fffffff]), 1);
        assert_eq!(
            model(
                22,
                &[0x7ffffff5, 0x7ffffff5, 0x7ffffff5, 0x7ffffff5, 10, 10, 2]
            ),
            0
        );
        assert_eq!(
            model(
                22,
                &[0x7ffffff6, 0x7ffffff5, 0x7ffffff5, 0x7ffffff5, 10, 10, 2]
            ),
            1
        );
        assert_eq!(
            model(
                23,
                &[
                    0xfffffffe, 0xfffffffe, 0x80000000, 0x80000000, 0x7fffffff, 0x80000000,
                    0x80000000, 0x7fffffff
                ]
            ),
            1
        );
        assert_eq!(
            model(
                23,
                &[0, 0, 0x80000000, 0x80000000, 0x7fffffff, 0x80000000, 0x80000000, 0x7fffffff]
            ),
            0
        );
        assert_eq!(
            model(24, &[0, 0, 0x80000000, 0x80000000, 0x7fffffff, 0x7fffffff]),
            1
        );
    }
    #[test]
    fn binary32_ceil_truncate_and_comparisons() {
        let (pc, oc) = program(16);
        let (pt, ot) = program(15);
        let (pl, ol) = program(17);
        let mut seed = 0x53A71911u32;
        for _ in 0..4000 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let a = seed;
            let x = f32::from_bits(a);
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let y = f32::from_bits(seed);
            let mut args = [0; 16];
            args[0] = a;
            args[1] = seed;
            let ceil = x.ceil();
            let want = if ceil.is_nan() {
                0x7fc00000
            } else {
                ceil.to_bits()
            };
            assert_eq!(pc.eval(&args, &oc)[0], want, "ceil {a:08x}");
            if x.is_finite() && x >= i32::MIN as f32 && x < (i32::MAX as f32) {
                assert_eq!(pt.eval(&args, &ot)[0], (x as i32) as u32, "trunc {a:08x}");
            }
            assert_eq!(pl.eval(&args, &ol)[0], u32::from(x < y));
        }
    }
}

impl Node {
    fn arity(&self) -> usize {
        match self.op {
            Op::Input(_) | Op::Lit(_) => 0,
            Op::Select => 3,
            _ => 2,
        }
    }
}

/// Encode the modeled integer operations using L's opcode definitions. Physical
/// temporary registers are reused after their SSA value's last use.
fn tgsi(code: &Code, outputs: &[V]) -> (String, u32, usize) {
    use std::fmt::Write;
    let mut live = vec![false; code.nodes.len()];
    let mut todo = outputs.to_vec();
    while let Some(v) = todo.pop() {
        if live[v] {
            continue;
        }
        live[v] = true;
        let n = &code.nodes[v];
        todo.extend_from_slice(&n.args[..n.arity()]);
    }
    let mut last = vec![0; code.nodes.len()];
    for (i, n) in code.nodes.iter().enumerate() {
        if live[i] {
            for &v in &n.args[..n.arity()] {
                last[v] = i;
            }
        }
    }
    for &v in outputs {
        last[v] = code.nodes.len();
    }
    let mut constants = Vec::<u32>::new();
    for (i, n) in code.nodes.iter().enumerate() {
        if live[i] {
            if let Op::Lit(k) = n.op {
                if !constants.contains(&k) {
                    constants.push(k);
                }
            }
        }
    }
    for k in [
        0,
        1,
        2,
        3,
        4,
        5,
        8,
        16,
        24,
        31,
        32,
        96,
        255,
        u32::MAX,
        0x3b808081,
    ] {
        if !constants.contains(&k) {
            constants.push(k);
        }
    }
    let kref = |k: u32| {
        let i = constants.iter().position(|&v| v == k).unwrap();
        format!("IMM[{}].{}", i / 4, ["x", "y", "z", "w"][i % 4])
    };
    let mut body = String::from("F2I TEMP[0].xy, IN[0]\n");
    let mut tokens = 3u32;
    let mut refs = vec![String::new(); code.nodes.len()];
    for (i, n) in code.nodes.iter().enumerate() {
        match n.op {
            Op::Input(j) => {
                refs[i] = if j < 16 {
                    format!("CONST[{}].{}", j / 4, ["x", "y", "z", "w"][j % 4])
                } else {
                    format!("TEMP[0].{}", if j == 16 { "x" } else { "y" })
                }
            }
            Op::Lit(k) if live[i] => refs[i] = kref(k),
            _ => {}
        }
    }
    let mut physical = vec![None; code.nodes.len()];
    let mut free = Vec::new();
    let mut next = 1usize;
    for (i, n) in code.nodes.iter().enumerate() {
        if let Some(d) = code.divisions.iter().find(|d| d.start == i) {
            let a = d.a.map(|v| refs[v].clone());
            let b = d.b.map(|v| refs[v].clone());
            let (registers, extra) = emit_division(&mut body, &a, &b, &kref, &mut free, &mut next);
            tokens += extra;
            for (&v, r) in d.out.iter().zip(registers) {
                refs[v] = format!("TEMP[{r}].x");
                if live[v] {
                    physical[v] = Some(r);
                } else {
                    free.push(r);
                }
            }
        }
        if code.divisions.iter().any(|d| i >= d.start && i < d.end) {
            continue;
        }
        if !live[i] {
            continue;
        }
        match n.op {
            Op::Input(j) => {
                refs[i] = if j < 16 {
                    format!("CONST[{}].{}", j / 4, ["x", "y", "z", "w"][j % 4])
                } else {
                    format!("TEMP[0].{}", if j == 16 { "x" } else { "y" })
                };
            }
            Op::Lit(k) => refs[i] = kref(k),
            _ => {
                let r = free.pop().unwrap_or_else(|| {
                    let r = next;
                    next += 1;
                    r
                });
                physical[i] = Some(r);
                refs[i] = format!("TEMP[{r}].x");
                let name = match n.op {
                    Op::Add => "UADD",
                    Op::Mul => "UMUL",
                    Op::And => "AND",
                    Op::Or => "OR",
                    Op::Xor => "XOR",
                    Op::Shl => "SHL",
                    Op::Shr => "USHR",
                    Op::Lt => "USLT",
                    Op::Eq => "USEQ",
                    Op::Select => "UCMP",
                    _ => unreachable!(),
                };
                write!(body, "{name} {}", refs[i]).unwrap();
                for &a in &n.args[..n.arity()] {
                    write!(body, ", {}", refs[a]).unwrap();
                }
                body.push('\n');
                tokens += 2 + n.arity() as u32;
                if matches!(n.op, Op::Lt | Op::Eq) {
                    writeln!(body, "AND {}, {}, {}", refs[i], refs[i], kref(1)).unwrap();
                    tokens += 4;
                }
            }
        }
        for &a in &n.args[..n.arity()] {
            if last[a] == i {
                if let Some(r) = physical[a].take() {
                    free.push(r);
                }
            }
        }
    }
    // Keep the result words live while selecting the requested one.
    let result = next;
    let condition = next + 1;
    let colour = next + 2;
    next += 3;
    writeln!(body, "MOV TEMP[{result}].x, {}", refs[outputs[0]]).unwrap();
    tokens += 3;
    for (j, &v) in outputs.iter().enumerate().skip(1) {
        writeln!(
            body,
            "USEQ TEMP[{condition}].x, CONST[4].x, {}",
            kref(j as u32)
        )
        .unwrap();
        tokens += 4;
        writeln!(
            body,
            "UCMP TEMP[{result}].x, TEMP[{condition}].x, {}, TEMP[{result}].x",
            refs[v]
        )
        .unwrap();
        tokens += 5;
    }
    for (component, shift) in [("x", 16), ("y", 8), ("z", 0), ("w", 24)] {
        writeln!(
            body,
            "USHR TEMP[{colour}].{component}, TEMP[{result}].x, {}",
            kref(shift)
        )
        .unwrap();
        writeln!(
            body,
            "AND TEMP[{colour}].{component}, TEMP[{colour}].{component}, {}",
            kref(255)
        )
        .unwrap();
        tokens += 8;
    }
    write!(
        body,
        "U2F TEMP[{colour}], TEMP[{colour}]\nMUL OUT[0], TEMP[{colour}], {}\nEND\n\0",
        kref(0x3b808081)
    )
    .unwrap();
    tokens += 3 + 4 + 1;
    let mut text = format!(
        "FRAG\nDCL IN[0], POSITION, LINEAR\nDCL OUT[0], COLOR\nDCL CONST[0..4]\nDCL TEMP[0..{}]\n",
        next - 1
    );
    tokens += 2 + 4 + 3 + 2 + 2;
    for (i, chunk) in constants.chunks(4).enumerate() {
        let mut k = [0; 4];
        k[..chunk.len()].copy_from_slice(chunk);
        writeln!(
            text,
            "IMM[{i}] UINT32 {{{}, {}, {}, {}}}",
            k[0], k[1], k[2], k[3]
        )
        .unwrap();
        tokens += 5;
    }
    text.push_str(&body);
    (text, tokens, next)
}

fn generated() -> (String, u32) {
    use std::fmt::Write;
    let mut pieces = Vec::new();
    let mut max_registers = 0;
    for r in 0..25 {
        let (p, o) = program(r);
        let (text, tokens, registers) = tgsi(&p, &o);
        println!(
            "routine={r} bytes={} tokens={tokens} registers={registers}",
            text.len()
        );
        max_registers = max_registers.max(registers);
        pieces.push((text, tokens));
    }
    let mut text=format!("FRAG\nDCL IN[0], POSITION, LINEAR\nDCL OUT[0], COLOR\nDCL CONST[0..4]\nDCL TEMP[0..{max_registers}]\n");
    let mut tokens = 13 + 7 * 5 + 1;
    for i in 0..7 {
        writeln!(
            text,
            "IMM[{i}] UINT32 {{{}, {}, {}, {}}}",
            i * 4,
            i * 4 + 1,
            i * 4 + 2,
            i * 4 + 3
        )
        .unwrap();
    }
    let mut bodies = String::new();
    let mut offset = 7;
    for (r, (piece, count)) in pieces.iter().enumerate() {
        let immediates = piece.lines().filter(|l| l.starts_with("IMM[")).count();
        for line in piece.lines().filter(|l| l.starts_with("IMM[")) {
            writeln!(text, "{}", renumber(line, offset)).unwrap();
        }
        writeln!(
            bodies,
            "USEQ TEMP[{max_registers}].x, CONST[4].y, IMM[{}].{}\nUIF TEMP[{max_registers}].x",
            r / 4,
            ["x", "y", "z", "w"][r % 4]
        )
        .unwrap();
        for line in piece.lines().skip_while(|l| !l.starts_with("F2I ")) {
            if line == "END" || line.contains('\0') {
                continue;
            }
            writeln!(bodies, "{}", renumber(line, offset)).unwrap();
        }
        bodies.push_str("ENDIF\n");
        tokens += count - 13 - 1 + 7;
        offset += immediates;
    }
    text.push_str(&bodies);
    text.push_str("END\n\0");

    let text = expand_sources(&text);
    let text = integer_words(&text, max_registers, &mut tokens);
    let (text, constant_registers) = integer_constants(&text, max_registers + 3, &mut tokens);
    // T:649-688: a flow instruction may carry one instruction-label token.
    tokens += text
        .lines()
        .filter(|l| l.starts_with("UIF ") || matches!(*l, "BGNLOOP" | "ENDLOOP" | "BRK"))
        .count() as u32;
    assert!(
        text.len().div_ceil(4) + 5 <= 65532,
        "combined shader exceeds the VirGL command size: {} bytes",
        text.len()
    );
    println!(
        "combined bytes={} tokens={tokens} registers={}",
        text.len(),
        max_registers + 3 + constant_registers
    );
    (text, tokens)
}

#[cfg(not(test))]
fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().expect("output directory or --pipelines");
    if first == "--pipelines" {
        pipelines::generate(std::path::Path::new(&args.next().expect("output directory")));
        return;
    }
    let destination = first;
    let destination = std::path::Path::new(&destination);
    std::fs::create_dir_all(destination).unwrap();
    let (text, tokens) = generated();
    std::fs::write(
        destination.join("profile.tgsi"),
        text.trim_end_matches('\0'),
    )
    .unwrap();
    std::fs::write(
        destination.join("mod.rs"),
        format!("// Generated by tools/gpu-profile/math.rs.\nuse super::{{pipe, Program}};\n\npub const FS: Program = Program {{\n    kind: pipe::SHADER_FRAGMENT,\n    text: include_bytes!(\"profile.tgsi\"),\n    tokens: {tokens},\n}};\n"),
    ).unwrap();
}

fn renumber(line: &str, offset: usize) -> String {
    let mut result = String::new();
    let mut rest = line;
    while let Some(at) = rest.find("IMM[") {
        result.push_str(&rest[..at + 4]);
        rest = &rest[at + 4..];
        let end = rest.find(']').unwrap();
        let index = rest[..end].parse::<usize>().unwrap();
        result.push_str(&(index + offset).to_string());
        rest = &rest[end..];
    }
    result.push_str(rest);
    result
}

fn allocate(free: &mut Vec<usize>, next: &mut usize) -> usize {
    free.pop().unwrap_or_else(|| {
        let r = *next;
        *next += 1;
        r
    })
}

/// Encode the model's restoring-division recurrence as a bounded loop. The arithmetic
/// model above unrolls that same recurrence for comparison with native u128.
fn emit_division(
    body: &mut String,
    a: &[String; 3],
    b: &[String; 3],
    k: &impl Fn(u32) -> String,
    free: &mut Vec<usize>,
    next: &mut usize,
) -> ([usize; 6], u32) {
    use std::fmt::Write;
    let result = core::array::from_fn(|_| allocate(free, next));
    let work: [usize; 12] = core::array::from_fn(|_| allocate(free, next));
    let q: [String; 3] = core::array::from_fn(|i| format!("TEMP[{}].x", result[i]));
    let r: [String; 3] = core::array::from_fn(|i| format!("TEMP[{}].x", result[i + 3]));
    let w: [String; 12] = work.map(|i| format!("TEMP[{i}].x"));
    let mut tokens = 0;
    let mut emit = |op: &str, d: &str, src: &[&str]| {
        write!(body, "{op} {d}").unwrap();
        for s in src {
            write!(body, ", {s}").unwrap();
        }
        body.push('\n');
        tokens += 2 + src.len() as u32;
    };
    let (z, one, two, k31, k96, all) = (k(0), k(1), k(2), k(31), k(96), k(u32::MAX));
    for dest in q.iter().chain(r.iter()) {
        emit("MOV", dest, &[&z]);
    }
    emit("MOV", &w[0], &[&k96]);
    drop(emit);
    body.push_str("BGNLOOP\n");
    tokens += 1;
    let mut emit = |op: &str, d: &str, src: &[&str]| {
        write!(body, "{op} {d}").unwrap();
        for s in src {
            write!(body, ", {s}").unwrap();
        }
        body.push('\n');
        tokens += 2 + src.len() as u32;
    };
    // w0 bit, w1 word index, w2 chosen dividend word / incoming bit.
    emit("UADD", &w[0], &[&w[0], &all]);
    emit("USHR", &w[1], &[&w[0], &k(5)]);
    emit("USEQ", &w[3], &[&w[1], &one]);
    emit("UCMP", &w[2], &[&w[3], &a[1], &a[0]]);
    emit("USEQ", &w[3], &[&w[1], &two]);
    emit("UCMP", &w[2], &[&w[3], &a[2], &w[2]]);
    emit("USHR", &w[2], &[&w[2], &w[0]]);
    emit("AND", &w[2], &[&w[2], &one]);
    emit("USHR", &w[4], &[&r[2], &k31]);
    for value in &r {
        emit("USHR", &w[3], &[value, &k31]);
        emit("SHL", value, &[value, &one]);
        emit("OR", value, &[value, &w[2]]);
        emit("MOV", &w[2], &[&w[3]]);
    }
    // Unsigned three-word comparison; w5 is the subtract mask.
    emit("USLT", &w[5], &[&r[0], &b[0]]);
    for i in 1..3 {
        emit("USEQ", &w[3], &[&r[i], &b[i]]);
        emit("USLT", &w[6], &[&r[i], &b[i]]);
        emit("UCMP", &w[5], &[&w[3], &w[5], &w[6]]);
    }
    emit("XOR", &w[5], &[&w[5], &all]);
    emit("OR", &w[5], &[&w[5], &w[4]]);
    emit("MOV", &w[7], &[&z]);
    for i in 0..3 {
        emit("XOR", &w[8], &[&b[i], &all]);
        emit("UADD", &w[8], &[&w[8], &one]);
        emit("UADD", &w[9], &[&r[i], &w[8]]);
        emit("USLT", &w[10], &[&r[i], &b[i]]);
        emit("USLT", &w[11], &[&w[9], &w[7]]);
        emit("OR", &w[10], &[&w[10], &w[11]]);
        emit("AND", &w[10], &[&w[10], &one]);
        emit("XOR", &w[8], &[&w[7], &all]);
        emit("UADD", &w[8], &[&w[8], &one]);
        emit("UADD", &w[9], &[&w[9], &w[8]]);
        emit("MOV", &w[7], &[&w[10]]);
        emit("UCMP", &r[i], &[&w[5], &w[9], &r[i]]);
    }
    emit("AND", &w[5], &[&w[5], &one]);
    emit("SHL", &w[5], &[&w[5], &w[0]]);
    for i in 0..3 {
        emit("USEQ", &w[3], &[&w[1], &k(i as u32)]);
        emit("AND", &w[3], &[&w[3], &w[5]]);
        emit("OR", &q[i], &[&q[i], &w[3]]);
    }
    emit("USEQ", &w[3], &[&w[0], &z]);
    drop(emit);
    writeln!(body, "UIF {}\nBRK\nENDIF\nENDLOOP", w[3]).unwrap();
    tokens += 2 + 1 + 1 + 1;
    free.extend(work);
    (result, tokens)
}

fn expand_sources(text: &str) -> String {
    fn expand(s: &str) -> String {
        if s.len() > 2
            && s.as_bytes()[s.len() - 2] == b'.'
            && b"xyzw".contains(&s.as_bytes()[s.len() - 1])
        {
            let c = s.chars().last().unwrap();
            format!("{}{c}{c}{c}", s)
        } else {
            s.to_string()
        }
    }
    let mut out = String::new();
    for line in text.lines() {
        if let Some(source) = line.strip_prefix("UIF ") {
            out.push_str("UIF ");
            out.push_str(&expand(source));
        } else if line.starts_with("DCL ") || line.starts_with("IMM[") {
            out.push_str(line);
        } else {
            let mut parts = line.split(", ");
            out.push_str(parts.next().unwrap_or(""));
            for source in parts {
                out.push_str(", ");
                out.push_str(&expand(source));
            }
        }
        if !line.ends_with('\0') {
            out.push('\n');
        }
    }
    out
}

/// Keep probe words in the integer ISA, including conditional selection and copies.
/// With m = (condition != 0 ? UINT_MAX : 0), b ^ ((a ^ b) & m) selects word bits.
fn integer_words(text: &str, control: usize, tokens: &mut u32) -> String {
    use std::fmt::Write;
    let mask = control + 1;
    let difference = control + 2;
    let old = format!("DCL TEMP[0..{control}]");
    let new = format!("DCL TEMP[0..{difference}]");
    let mut out = String::new();
    for line in text.lines() {
        if line == old {
            out.push_str(&new);
        } else if let Some(args) = line.strip_prefix("UCMP ") {
            let p: Vec<_> = args.split(", ").collect();
            assert_eq!(p.len(), 4);
            writeln!(out, "USNE TEMP[{mask}].x, {}, IMM[0].xxxx", p[1]).unwrap();
            writeln!(out, "XOR TEMP[{difference}].x, {}, {}", p[2], p[3]).unwrap();
            writeln!(
                out,
                "AND TEMP[{difference}].x, TEMP[{difference}].xxxx, TEMP[{mask}].xxxx"
            )
            .unwrap();
            write!(out, "XOR {}, TEMP[{difference}].xxxx, {}", p[0], p[3]).unwrap();
            *tokens += 11;
        } else if let Some(args) = line.strip_prefix("MOV ") {
            write!(out, "UADD {args}, IMM[0].xxxx").unwrap();
            *tokens += 1;
        } else {
            out.push_str(line);
        }
        if !line.ends_with('\0') {
            out.push('\n');
        }
    }
    out
}

/// Keep NaN-shaped literal words out of the renderer's floating constant folding.
/// The encoder supplies CONST[4].z as integer 0. Starting from that uniform,
/// construct each such word from two finite 16-bit immediates using L's integer ISA.
fn integer_constants(text: &str, first: usize, tokens: &mut u32) -> (String, usize) {
    use std::fmt::Write;
    let mut values = Vec::new();
    let mut replacements = Vec::new();
    let mut immediates = 0;
    let mut declarations = String::new();
    let mut body = String::new();
    for line in text.trim_end_matches('\0').lines() {
        if let Some(rest) = line.strip_prefix("IMM[") {
            let end = rest.find(']').unwrap();
            let index = rest[..end].parse::<usize>().unwrap();
            immediates = immediates.max(index + 1);
            let start = line.find('{').unwrap();
            let end = line.find('}').unwrap();
            let mut words: Vec<u32> = line[start + 1..end]
                .split(", ")
                .map(|s| s.parse().unwrap())
                .collect();
            for (component, word) in words.iter_mut().enumerate() {
                if *word & 0x7f800000 == 0x7f800000 && *word & 0x7fffff != 0 {
                    let slot = values.iter().position(|v| v == word).unwrap_or_else(|| {
                        values.push(*word);
                        values.len() - 1
                    });
                    let c = ["xxxx", "yyyy", "zzzz", "wwww"][component];
                    replacements.push((
                        format!("IMM[{index}].{c}"),
                        format!("TEMP[{}].xxxx", first + slot),
                    ));
                    *word = 0;
                }
            }
            writeln!(
                declarations,
                "{}{{{}, {}, {}, {}}}",
                &line[..start],
                words[0],
                words[1],
                words[2],
                words[3]
            )
            .unwrap();
        } else if line.starts_with("FRAG") || line.starts_with("DCL") {
            writeln!(declarations, "{line}").unwrap();
        } else {
            writeln!(body, "{line}").unwrap();
        }
    }
    for (old, new) in replacements {
        body = body.replace(&old, &new);
    }
    let mut initialization = String::new();
    for (i, word) in values.iter().enumerate() {
        let immediate = immediates + i;
        writeln!(
            declarations,
            "IMM[{immediate}] UINT32 {{{}, {}, 16, 0}}",
            word >> 16,
            word & 65535
        )
        .unwrap();
        let r = first + i;
        writeln!(
            initialization,
            "UADD TEMP[{r}].x, CONST[4].zzzz, IMM[{immediate}].xxxx"
        )
        .unwrap();
        writeln!(
            initialization,
            "SHL TEMP[{r}].x, TEMP[{r}].xxxx, IMM[{immediate}].zzzz"
        )
        .unwrap();
        writeln!(
            initialization,
            "OR TEMP[{r}].x, TEMP[{r}].xxxx, IMM[{immediate}].yyyy"
        )
        .unwrap();
        *tokens += 5 + 3 * 4;
    }
    let old = format!("DCL TEMP[0..{}]", first - 1);
    let new = format!("DCL TEMP[0..{}]", first + values.len() - 1);
    declarations = declarations.replace(&old, &new);
    declarations.push_str(&initialization);
    declarations.push_str(&body);
    declarations.push('\0');
    (declarations, values.len())
}

#[cfg(test)]
#[path = "emitted_tests.rs"]
mod emitted_tests;

mod pipelines;
