//! Double-double arithmetic: a value hi + lo with |lo| <= ulp(hi)/2, about 106 bits.
//! The transcendental functions work in it so that the final rounding to double is the
//! only significant error.
//!
//! Addition recovers the rounding residual by subtracting the rounded partial
//! sums. Multiplication uses our own fma to recover a*b-rounded(a*b), without
//! a splitting multiplier that can overflow. Finite intermediates and nearest
//! rounding are required for error-free double-double arithmetic.

#[derive(Clone, Copy, Debug)]
pub(crate) struct DD {
    pub hi: f64,
    pub lo: f64,
}

/// s + e == a + b exactly.
#[inline]
pub(crate) fn two_sum(a: f64, b: f64) -> DD {
    let s = a + b;
    if !s.is_finite() { return DD::from(s); }
    let bb = s - a;
    DD { hi: s, lo: (a - (s - bb)) + (b - bb) }
}

/// As two_sum, for |a| >= |b|.
#[inline]
fn quick_two_sum(a: f64, b: f64) -> DD {
    let s = a + b;
    if !s.is_finite() { return DD::from(s); }
    DD { hi: s, lo: b - (s - a) }
}

/// p + e == a * b exactly when the residual is representable.
#[inline]
pub(crate) fn two_prod(a: f64, b: f64) -> DD {
    let p = a * b;
    if !p.is_finite() { return DD::from(p); }
    // Residual underflow is an internal approximation detail, not a range
    // error of the public function using this product. Keep caller errno.
    let saved = crate::pthread::errno();
    let lo = super::basic::fma(a, b, -p);
    crate::pthread::set_errno(saved);
    DD { hi: p, lo }
}

impl DD {
    pub(crate) const fn new(hi: f64, lo: f64) -> DD {
        DD { hi, lo }
    }

    pub(crate) const fn from(x: f64) -> DD {
        DD { hi: x, lo: 0.0 }
    }

    pub(crate) fn to_f64(self) -> f64 {
        self.hi + self.lo
    }

    pub(crate) fn neg(self) -> DD {
        DD { hi: -self.hi, lo: -self.lo }
    }

    pub(crate) fn add(self, o: DD) -> DD {
        let s = two_sum(self.hi, o.hi);
        let t = two_sum(self.lo, o.lo);
        let s = quick_two_sum(s.hi, s.lo + t.hi);
        quick_two_sum(s.hi, s.lo + t.lo)
    }

    pub(crate) fn add_f(self, b: f64) -> DD {
        let s = two_sum(self.hi, b);
        quick_two_sum(s.hi, s.lo + self.lo)
    }

    pub(crate) fn sub(self, o: DD) -> DD {
        self.add(o.neg())
    }

    pub(crate) fn mul(self, o: DD) -> DD {
        let p = two_prod(self.hi, o.hi);
        if !p.hi.is_finite() { return p; }
        quick_two_sum(p.hi, p.lo + (self.hi * o.lo + self.lo * o.hi))
    }

    pub(crate) fn mul_f(self, b: f64) -> DD {
        let p = two_prod(self.hi, b);
        if !p.hi.is_finite() { return p; }
        quick_two_sum(p.hi, p.lo + self.lo * b)
    }

    pub(crate) fn sqr(self) -> DD {
        self.mul(self)
    }

    pub(crate) fn div(self, o: DD) -> DD {
        let q1 = self.hi / o.hi;
        let r = self.sub(o.mul_f(q1));
        let q2 = r.hi / o.hi;
        let r = r.sub(o.mul_f(q2));
        let q3 = r.hi / o.hi;
        quick_two_sum(q1, q2).add_f(q3)
    }

    pub(crate) fn div_f(self, b: f64) -> DD {
        self.div(DD::from(b))
    }

    pub(crate) fn recip(self) -> DD {
        DD::from(1.0).div(self)
    }

    /// One Newton step from the double square root.
    pub(crate) fn sqrt(self) -> DD {
        if self.hi <= 0.0 {
            return DD::from(0.0);
        }
        let s = super::basic::sqrt_exact(self.hi);
        let e = self.sub(two_prod(s, s));
        DD::from(s).add_f(e.hi / (2.0 * s))
    }

    /// Multiplies by 2^n; exact while the result stays normal.
    pub(crate) fn scale(self, n: i32) -> DD {
        // Keep each scaling factor normal, including normalization of the
        // top binade (n=-1023); pow2(-1023) used to encode zero.
        if n < -1022 { return self.scale(-1022).scale(n + 1022); }
        if n > 1023 { return self.scale(1023).scale(n - 1023); }
        let f = super::basic::pow2(n);
        DD { hi: self.hi * f, lo: self.lo * f }
    }
}
