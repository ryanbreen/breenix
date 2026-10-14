/*
 * libc suite, math category: <math.h> and <fenv.h> as ISO C17 (with Annex F, IEC 60559)
 * and POSIX.1-2024 specify them. Inputs are volatile so the compiler cannot evaluate a
 * call at build time instead of the library at run time.
 */
#define _XOPEN_SOURCE 800
#include "case.h"

#include <errno.h>
#include <fenv.h>
#include <float.h>
#include <limits.h>
#include <math.h>

/* The error a call should report: errno when math_errhandling has MATH_ERRNO, the
 * floating-point exception when it has MATH_ERREXCEPT. */
static const char *math_error(const char *what, double got, double want, int err, int except) {
    if (!h_dbl_same(got, want))
        return fail_dbl(what, got, want);
    if (!(math_errhandling & (MATH_ERRNO | MATH_ERREXCEPT)))
        return fail_text("math_errhandling has neither MATH_ERRNO nor MATH_ERREXCEPT");
    if ((math_errhandling & MATH_ERRNO) && errno != err)
        return fail_errno(what, errno, err);
    if ((math_errhandling & MATH_ERREXCEPT) && except && !fetestexcept(except))
        return fail_text(what);
    return PASS;
}

#define MATH_ERR(what, expr, want, err, except) do { errno = 0; feclearexcept(FE_ALL_EXCEPT); \
    double r_ = (expr); const char *m_ = math_error((what), r_, (want), (err), (except)); \
    if (m_) return m_; } while (0)

static volatile double zero = 0.0, nzero = -0.0, one = 1.0, two = 2.0, inf = INFINITY;

static const char *sqrt_values(void) {
    volatile double four = 4.0;
    EXPECT_DBL("sqrt(4)", sqrt(four), 2.0);
    EXPECT_DBL("sqrt(-0)", sqrt(nzero), -0.0);
    EXPECT_DBL("sqrt(inf)", sqrt(inf), INFINITY);
    EXPECT_DBL("sqrt(2) correctly rounded", sqrt(two), 1.4142135623730951);
    return PASS;
}

static const char *sqrt_domain(void) {
    MATH_ERR("sqrt(-1): NaN, EDOM and FE_INVALID", sqrt(-one), NAN, EDOM, FE_INVALID);
    return PASS;
}

static const char *errhandling(void) {
    EXPECT_INT("math_errhandling", math_errhandling, MATH_ERRNO | MATH_ERREXCEPT);
    return PASS;
}

static const char *log_errors(void) {
    MATH_ERR("log(0): -HUGE_VAL, ERANGE and FE_DIVBYZERO", log(zero), -HUGE_VAL, ERANGE, FE_DIVBYZERO);
    MATH_ERR("log(-0): -HUGE_VAL, ERANGE and FE_DIVBYZERO", log(nzero), -HUGE_VAL, ERANGE, FE_DIVBYZERO);
    MATH_ERR("log(-1): NaN, EDOM and FE_INVALID", log(-one), NAN, EDOM, FE_INVALID);
    MATH_ERR("log10(0): -HUGE_VAL, ERANGE and FE_DIVBYZERO", log10(zero), -HUGE_VAL, ERANGE, FE_DIVBYZERO);
    return PASS;
}

static const char *exp_range(void) {
    volatile double big = 1000.0;
    MATH_ERR("exp(1000): HUGE_VAL, ERANGE and FE_OVERFLOW", exp(big), HUGE_VAL, ERANGE, FE_OVERFLOW);
    EXPECT_DBL("exp(-1000)", exp(-big), 0.0);
    EXPECT_DBL("exp(-inf)", exp(-inf), 0.0);
    EXPECT_DBL("exp(inf)", exp(inf), INFINITY);
    return PASS;
}

static const char *pow_special(void) {
    volatile double exponent = -1075.0;
    fesetround(FE_DOWNWARD);
    EXPECT_DBL("pow(-2,-1075) downward", pow(-two, exponent), -DBL_TRUE_MIN);
    fesetround(FE_UPWARD);
    EXPECT_DBL("pow(-2,-1075) upward", pow(-two, exponent), -0.0);
    fesetround(FE_TONEAREST);
    volatile double nan = NAN, m1 = -1.0, m2 = -2.0, three = 3.0, ten = 10.0, half = 0.5, four = 4.0;
    EXPECT_DBL("pow(NaN, 0)", pow(nan, zero), 1.0);
    EXPECT_DBL("pow(NaN, -0)", pow(nan, nzero), 1.0);
    EXPECT_DBL("pow(1, NaN)", pow(one, nan), 1.0);
    EXPECT_DBL("pow(-1, inf)", pow(m1, inf), 1.0);
    EXPECT_DBL("pow(-1, -inf)", pow(m1, -inf), 1.0);
    EXPECT_DBL("pow(2, 10)", pow(two, ten), 1024.0);
    EXPECT_DBL("pow(-2, 3)", pow(m2, three), -8.0);
    EXPECT_DBL("pow(4, 0.5)", pow(four, half), 2.0);
    EXPECT_DBL("pow(-0, 3)", pow(nzero, three), -0.0);
    EXPECT_DBL("pow(0.5, inf)", pow(half, inf), 0.0);
    return PASS;
}

static const char *pow_errors(void) {
    volatile double extreme = 1e308;
    MATH_ERR("pow(2,1e308) overflows rather than NaN", pow(two, extreme), INFINITY, ERANGE, FE_OVERFLOW);
    volatile double m8 = -8.0, third = 1.0 / 3.0, ten = 10.0, big = 400.0;
    MATH_ERR("pow(-8, 1/3): NaN, EDOM and FE_INVALID", pow(m8, third), NAN, EDOM, FE_INVALID);
    MATH_ERR("pow(0, -1): HUGE_VAL, ERANGE and FE_DIVBYZERO", pow(zero, -one), HUGE_VAL, ERANGE, FE_DIVBYZERO);
    MATH_ERR("pow(-0, -1): -HUGE_VAL, ERANGE and FE_DIVBYZERO", pow(nzero, -one), -HUGE_VAL, ERANGE, FE_DIVBYZERO);
    MATH_ERR("pow(10, 400): HUGE_VAL, ERANGE and FE_OVERFLOW", pow(ten, big), HUGE_VAL, ERANGE, FE_OVERFLOW);
    return PASS;
}

static const char *fmod_values(void) {
    volatile double a = 5.5, three = 3.0;
    EXPECT_DBL("fmod(5.5, 2)", fmod(a, two), 1.5);
    EXPECT_DBL("fmod(-5.5, 2)", fmod(-a, two), -1.5);
    EXPECT_DBL("fmod(5.5, -2)", fmod(a, -two), 1.5);
    EXPECT_DBL("fmod(3, inf)", fmod(three, inf), 3.0);
    EXPECT_DBL("fmod(-0, 3)", fmod(nzero, three), -0.0);
    EXPECT_DBL("fmod(6, 3)", fmod(three * two, three), 0.0);
    return PASS;
}

static const char *fmod_domain(void) {
    MATH_ERR("fmod(inf, 2): NaN, EDOM and FE_INVALID", fmod(inf, two), NAN, EDOM, FE_INVALID);
    MATH_ERR("fmod(1, 0): NaN, EDOM and FE_INVALID", fmod(one, zero), NAN, EDOM, FE_INVALID);
    return PASS;
}

static const char *remainder_ties(void) {
    volatile double five = 5.0, seven = 7.0;
    int q = 0;
    EXPECT_DBL("remainder(5, 2)", remainder(five, two), 1.0);
    EXPECT_DBL("remainder(7, 2)", remainder(seven, two), -1.0);
    EXPECT_DBL("remquo(7, 2)", remquo(seven, two, &q), -1.0);
    EXPECT_INT("remquo(7, 2) quotient bits", q & 7, 4);
    EXPECT_DBL("remquo(-7, 2)", remquo(-seven, two, &q), 1.0);
    CHECK(q < 0 && ((-q) & 7) == 4, "remquo(-7, 2) quotient is not congruent to -4");
    return PASS;
}

static const char *frexp_split(void) {
    volatile double eight = 8.0, m3 = -3.0, tiny = 0x1p-1074;
    int e = 99;
    EXPECT_DBL("frexp(8)", frexp(eight, &e), 0.5);
    EXPECT_INT("frexp(8) exponent", e, 4);
    EXPECT_DBL("frexp(-3)", frexp(m3, &e), -0.75);
    EXPECT_INT("frexp(-3) exponent", e, 2);
    EXPECT_DBL("frexp(-0)", frexp(nzero, &e), -0.0);
    EXPECT_INT("frexp(-0) exponent", e, 0);
    EXPECT_DBL("frexp of the smallest subnormal", frexp(tiny, &e), 0.5);
    EXPECT_INT("frexp of the smallest subnormal: exponent", e, -1073);
    return PASS;
}

static const char *ldexp_scale(void) {
    volatile double q = 0.75;
    EXPECT_DBL("ldexp(0.75, 2)", ldexp(q, 2), 3.0);
    EXPECT_DBL("ldexp(1, -1074)", ldexp(one, -1074), 0x1p-1074);
    EXPECT_DBL("ldexp(-0, 5)", ldexp(nzero, 5), -0.0);
    MATH_ERR("ldexp(1, 2000): HUGE_VAL, ERANGE and FE_OVERFLOW", ldexp(one, 2000), HUGE_VAL, ERANGE, FE_OVERFLOW);
    return PASS;
}

static const char *modf_parts(void) {
    volatile double a = 3.75;
    double i = 0;
    EXPECT_DBL("modf(3.75)", modf(a, &i), 0.75);
    EXPECT_DBL("modf(3.75) integral part", i, 3.0);
    EXPECT_DBL("modf(-3.75)", modf(-a, &i), -0.75);
    EXPECT_DBL("modf(-3.75) integral part", i, -3.0);
    EXPECT_DBL("modf(-inf)", modf(-inf, &i), -0.0);
    EXPECT_DBL("modf(-inf) integral part", i, -INFINITY);
    return PASS;
}

static const char *round_functions(void) {
    feclearexcept(FE_ALL_EXCEPT);
    EXPECT_DBL("round(inf)", round(inf), INFINITY);
    EXPECT_DBL("roundf(-inf)", roundf(-INFINITY), -INFINITY);
    CHECK(!fetestexcept(FE_INVALID), "round of infinity raised invalid");
    volatile double a = 2.5, h = 0.5, b = 1.7;
    EXPECT_DBL("round(2.5)", round(a), 3.0);
    EXPECT_DBL("round(-2.5)", round(-a), -3.0);
    EXPECT_DBL("round(-0.4)", round(-0.4 * one), -0.0);
    EXPECT_DBL("ceil(-0.5)", ceil(-h), -0.0);
    EXPECT_DBL("floor(-0.5)", floor(-h), -1.0);
    EXPECT_DBL("floor(-0)", floor(nzero), -0.0);
    EXPECT_DBL("trunc(-1.7)", trunc(-b), -1.0);
    EXPECT_DBL("trunc(-0.7)", trunc(-0.7 * one), -0.0);
    EXPECT_DBL("ceil(inf)", ceil(inf), INFINITY);
    return PASS;
}

static const char *rint_modes(void) {
    volatile double a = 1.1, b = 1.9, c = 2.5;
    const char *bad = PASS;
    if (fesetround(FE_UPWARD) == 0 && rint(a) != 2.0)
        bad = "rint(1.1) under FE_UPWARD is not 2";
    if (!bad && fesetround(FE_DOWNWARD) == 0 && rint(-a) != -2.0)
        bad = "rint(-1.1) under FE_DOWNWARD is not -2";
    if (!bad && fesetround(FE_TOWARDZERO) == 0 && rint(-b) != -1.0)
        bad = "rint(-1.9) under FE_TOWARDZERO is not -1";
    if (!bad && fesetround(FE_TONEAREST) == 0 && rint(c) != 2.0)
        bad = "rint(2.5) under FE_TONEAREST is not 2 (ties to even)";
    if (!bad && fesetround(FE_UPWARD) == 0 && nearbyint(-a) != -1.0)
        bad = "nearbyint(-1.1) under FE_UPWARD is not -1";
    fesetround(FE_TONEAREST);
    return bad ? fail_text(bad) : PASS;
}

static const char *fesetround_query(void) {
    volatile double three = 3.0;
    EXPECT_INT("fegetround at startup", fegetround(), FE_TONEAREST);
    EXPECT_INT("fesetround(FE_DOWNWARD)", fesetround(FE_DOWNWARD), 0);
    EXPECT_INT("fegetround after FE_DOWNWARD", fegetround(), FE_DOWNWARD);
    volatile double down = one / three;
    CHECK(fesetround(-12345) != 0, "fesetround of an invalid mode succeeded");
    EXPECT_INT("fegetround after an invalid fesetround", fegetround(), FE_DOWNWARD);
    fesetround(FE_UPWARD);
    volatile double up = one / three;
    fesetround(FE_TONEAREST);
    CHECK(up > down, "1/3 rounded upward is not above 1/3 rounded downward");
    return PASS;
}

static const char *lround_lrint(void) {
    errno = 0;
    (void)lrint(inf);
    EXPECT_ERRNO("lrint(inf)", EDOM);
    errno = 0;
    (void)llroundf(INFINITY);
    EXPECT_ERRNO("llroundf(inf)", EDOM);
    volatile double a = 2.5, b = 3.5, h = 0.5, big = 1e30;
    EXPECT_INT("lround(2.5)", lround(a), 3);
    EXPECT_INT("lround(-2.5)", lround(-a), -3);
    EXPECT_INT("llround(-0.5)", llround(-h), -1);
    EXPECT_INT("lrint(2.5)", lrint(a), 2);
    EXPECT_INT("llrint(3.5)", llrint(b), 4);
    feclearexcept(FE_ALL_EXCEPT);
    (void)lround(big);
    CHECK(fetestexcept(FE_INVALID), "lround(1e30) did not raise FE_INVALID");
    return PASS;
}

static const char *fe_exceptions(void) {
    volatile double r;
    feclearexcept(FE_ALL_EXCEPT);
    EXPECT_INT("fetestexcept after feclearexcept", fetestexcept(FE_ALL_EXCEPT), 0);
    r = one / zero;
    CHECK(fetestexcept(FE_DIVBYZERO), "1/0 did not raise FE_DIVBYZERO");
    r = zero / zero;
    CHECK(fetestexcept(FE_INVALID), "0/0 did not raise FE_INVALID");
    feclearexcept(FE_ALL_EXCEPT);
    volatile double max = DBL_MAX;
    r = max * two;
    CHECK(fetestexcept(FE_OVERFLOW) && fetestexcept(FE_INEXACT), "DBL_MAX*2 did not raise FE_OVERFLOW and FE_INEXACT");
    CHECK(!fetestexcept(FE_DIVBYZERO), "DBL_MAX*2 raised FE_DIVBYZERO");
    (void)r;
    EXPECT_INT("feraiseexcept(FE_UNDERFLOW)", feraiseexcept(FE_UNDERFLOW), 0);
    CHECK(fetestexcept(FE_UNDERFLOW), "feraiseexcept(FE_UNDERFLOW) left it clear");
    EXPECT_INT("feclearexcept(FE_UNDERFLOW)", feclearexcept(FE_UNDERFLOW), 0);
    CHECK(!fetestexcept(FE_UNDERFLOW) && fetestexcept(FE_OVERFLOW), "feclearexcept cleared the wrong flags");
    return PASS;
}

static const char *feholdexcept_env(void) {
    fenv_t env;
    fexcept_t flag;
    feclearexcept(FE_ALL_EXCEPT);
    feraiseexcept(FE_INVALID);
    EXPECT_INT("fegetexceptflag", fegetexceptflag(&flag, FE_INVALID), 0);
    EXPECT_INT("feholdexcept", feholdexcept(&env), 0);
    CHECK(!fetestexcept(FE_INVALID), "feholdexcept did not clear the flags");
    feraiseexcept(FE_DIVBYZERO);
    EXPECT_INT("feupdateenv", feupdateenv(&env), 0);
    CHECK(fetestexcept(FE_INVALID), "feupdateenv did not restore FE_INVALID");
    CHECK(fetestexcept(FE_DIVBYZERO), "feupdateenv dropped the exception raised while held");
    feclearexcept(FE_ALL_EXCEPT);
    EXPECT_INT("fesetexceptflag", fesetexceptflag(&flag, FE_INVALID), 0);
    CHECK(fetestexcept(FE_INVALID), "fesetexceptflag did not set FE_INVALID");
    return PASS;
}

static const char *fpclassify_kinds(void) {
    volatile double sub = DBL_MIN / 4, nan = NAN;
    EXPECT_INT("fpclassify(NaN)", fpclassify(nan), FP_NAN);
    EXPECT_INT("fpclassify(inf)", fpclassify(inf), FP_INFINITE);
    EXPECT_INT("fpclassify(DBL_MIN/4)", fpclassify(sub), FP_SUBNORMAL);
    EXPECT_INT("fpclassify(-0)", fpclassify(nzero), FP_ZERO);
    EXPECT_INT("fpclassify(1)", fpclassify(one), FP_NORMAL);
    CHECK(isnan(nan) && !isnan(inf), "isnan");
    CHECK(isinf(-inf) && !isinf(DBL_MAX), "isinf");
    CHECK(isfinite(sub) && !isfinite(inf), "isfinite");
    CHECK(isnormal(one) && !isnormal(sub) && !isnormal(zero), "isnormal");
    return PASS;
}

static const char *nan_quiet_compare(void) {
    volatile double n = nan("");
    CHECK(isnan(n), "nan(\"\") is not a NaN");
    CHECK(n != n, "a NaN compares equal to itself");
    feclearexcept(FE_ALL_EXCEPT);
    CHECK(isunordered(n, one), "isunordered(NaN, 1) is false");
    CHECK(!isless(n, one) && !isgreater(n, one) && !islessgreater(n, one), "a quiet comparison of NaN is true");
    CHECK(!fetestexcept(FE_INVALID), "a quiet comparison macro raised FE_INVALID");
    return PASS;
}

static const char *copysign_signbit(void) {
    EXPECT_DBL("copysign(1, -0)", copysign(one, nzero), -1.0);
    EXPECT_DBL("copysign(-2, 0)", copysign(-two, zero), 2.0);
    EXPECT_DBL("fabs(-0)", fabs(nzero), 0.0);
    EXPECT_DBL("fabs(-inf)", fabs(-inf), INFINITY);
    CHECK(signbit(nzero) && !signbit(zero), "signbit does not tell -0 from +0");
    CHECK(signbit(copysign(NAN, -1.0)), "signbit of a negative NaN is clear");
    return PASS;
}

static const char *hypot_no_overflow(void) {
    fesetround(FE_UPWARD);
    MATH_ERR("hypot(DBL_MAX,1) upward", hypot(DBL_MAX * one, one), INFINITY, ERANGE, FE_OVERFLOW);
    fesetround(FE_TONEAREST);
    volatile double three = 3.0, four = 4.0, big = 1e300, nan = NAN;
    EXPECT_DBL("hypot(3, 4)", hypot(three, four), 5.0);
    EXPECT_DBL("hypot(inf, NaN)", hypot(inf, nan), INFINITY);
    EXPECT_NEAR("hypot(1e300, 1e300)", hypot(big, big), 1.4142135623730951e300, 1e285);
    return PASS;
}

static const char *trig_zeros(void) {
    EXPECT_DBL("sin(-0)", sin(nzero), -0.0);
    EXPECT_DBL("cos(0)", cos(zero), 1.0);
    EXPECT_DBL("tan(-0)", tan(nzero), -0.0);
    EXPECT_DBL("atan2(+0, -0)", atan2(zero, nzero), M_PI);
    EXPECT_DBL("atan2(-0, -0)", atan2(nzero, nzero), -M_PI);
    EXPECT_DBL("atan2(-0, +0)", atan2(nzero, zero), -0.0);
    EXPECT_DBL("atan2(1, 0)", atan2(one, zero), M_PI_2);
    EXPECT_DBL("atan(inf)", atan(inf), M_PI_2);
    return PASS;
}

static const char *trig_values(void) {
    volatile double least = DBL_TRUE_MIN, large = 1e308;
    EXPECT_DBL("atan true min", atan(least), DBL_TRUE_MIN);
    EXPECT_DBL("asin true min", asin(least), DBL_TRUE_MIN);
    EXPECT_NEAR("atan(1e308)", atan(large), M_PI_2, 3e-16);
    CHECK(atan2(one,large) > 0.0 && isfinite(atan2(one,large)), "atan2(1,1e308) is not positive finite");
    volatile double pi6 = M_PI / 6, pi3 = M_PI / 3, big = 1e22;
    EXPECT_NEAR("sin(pi/6)", sin(pi6), 0.5, 2e-16);
    EXPECT_NEAR("cos(pi/3)", cos(pi3), 0.5, 2e-16);
    EXPECT_NEAR("asin(1)", asin(one), M_PI_2, 3e-16);
    EXPECT_NEAR("acos(-1)", acos(-one), M_PI, 5e-16);
    EXPECT_NEAR("sin(1e22) with full argument reduction", sin(big), -0.8522008497671888, 2e-16);
    EXPECT_NEAR("tanh(1)", tanh(one), 0.7615941559557649, 2e-16);
    return PASS;
}

static const char *trig_domain(void) {
    MATH_ERR("sin(inf): NaN, EDOM and FE_INVALID", sin(inf), NAN, EDOM, FE_INVALID);
    MATH_ERR("cos(-inf): NaN, EDOM and FE_INVALID", cos(-inf), NAN, EDOM, FE_INVALID);
    MATH_ERR("acos(2): NaN, EDOM and FE_INVALID", acos(two), NAN, EDOM, FE_INVALID);
    MATH_ERR("asin(-2): NaN, EDOM and FE_INVALID", asin(-two), NAN, EDOM, FE_INVALID);
    return PASS;
}

static const char *exp_log_values(void) {
    volatile double large = 1e308;
    EXPECT_NEAR("log(1e308)", log(large), 709.1962086421661, 2e-13);
    EXPECT_NEAR("log2(DBL_MAX)", log2(DBL_MAX * one), 1024.0, 3e-13);
    EXPECT_NEAR("log10(1e308)", log10(large), 308.0, 1e-13);
    volatile double ten = 10.0, k = 1000.0, e1024 = 1024.0;
    EXPECT_DBL("exp(0)", exp(zero), 1.0);
    EXPECT_DBL("log(1)", log(one), 0.0);
    EXPECT_DBL("exp2(10)", exp2(ten), 1024.0);
    EXPECT_NEAR("log2(1024)", log2(e1024), 10.0, 2e-15);
    EXPECT_NEAR("log10(1000)", log10(k), 3.0, 5e-16);
    EXPECT_NEAR("log(e)", log(M_E), 1.0, 2e-16);
    EXPECT_NEAR("cbrt(-27)", cbrt(-27.0 * one), -3.0, 5e-16);
    EXPECT_DBL("cbrt(-0)", cbrt(nzero), -0.0);
    return PASS;
}

static const char *log1p_expm1(void) {
    volatile double least = DBL_TRUE_MIN;
    EXPECT_DBL("log1p true min", log1p(least), DBL_TRUE_MIN);
    EXPECT_DBL("asinh true min", asinh(least), DBL_TRUE_MIN);
    for (int i = 0; i < 3; ++i) {
        const int modes[] = {FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};
        fesetround(modes[i]);
        EXPECT_DBL("expm1(-inf) exact", expm1(-inf), -1.0);
        EXPECT_DBL("tanh(inf) exact", tanh(inf), 1.0);
        EXPECT_DBL("erf(-inf) exact", erf(-inf), -1.0);
    }
    fesetround(FE_TONEAREST);
    EXPECT_DBL("expm1(-0)", expm1(nzero), -0.0);
    EXPECT_DBL("log1p(-0)", log1p(nzero), -0.0);
    volatile double tiny = 1e-20;
    EXPECT_DBL("log1p(1e-20) keeps the small argument", log1p(tiny), 1e-20);
    EXPECT_DBL("expm1(1e-20) keeps the small argument", expm1(tiny), 1e-20);
    MATH_ERR("log1p(-1): -HUGE_VAL, ERANGE and FE_DIVBYZERO", log1p(-one), -HUGE_VAL, ERANGE, FE_DIVBYZERO);
    MATH_ERR("log1p(-2): NaN, EDOM and FE_INVALID", log1p(-two), NAN, EDOM, FE_INVALID);
    return PASS;
}

static const char *float_functions(void) {
    volatile float ninety = 90.0f;
    fesetround(FE_DOWNWARD);
    MATH_ERR("expf(90) downward range error", expf(ninety), FLT_MAX, ERANGE, FE_OVERFLOW);
    fesetround(FE_TONEAREST);
    volatile float f2 = 2.0f, a = 5.5f, b = 1.5f, big = 1000.0f;
    EXPECT_DBL("sqrtf(2)", (double)sqrtf(f2), (double)1.41421356f);
    EXPECT_DBL("floorf(-1.5)", (double)floorf(-b), -2.0);
    EXPECT_DBL("fmodf(5.5, 2)", (double)fmodf(a, f2), 1.5);
    EXPECT_DBL("ldexpf(1, -149)", (double)ldexpf(1.0f, -149), (double)0x1p-149f);
    errno = 0;
    float r = expf(big);
    EXPECT_DBL("expf(1000)", (double)r, (double)HUGE_VALF);
    if (math_errhandling & MATH_ERRNO)
        EXPECT_ERRNO("expf(1000)", ERANGE);
    return PASS;
}

static const char *gamma_values(void) {
    volatile double five = 5.0, half = 0.5;
    EXPECT_NEAR("tgamma(5)", tgamma(five), 24.0, 1e-13);
    EXPECT_NEAR("tgamma(0.5)", tgamma(half), 1.7724538509055159, 1e-15);
    EXPECT_DBL("lgamma(1)", lgamma(one), 0.0);
    EXPECT_DBL("lgamma(2)", lgamma(two), 0.0);
    MATH_ERR("tgamma(+0): HUGE_VAL, ERANGE and FE_DIVBYZERO", tgamma(zero), HUGE_VAL, ERANGE, FE_DIVBYZERO);
    MATH_ERR("tgamma(-1): NaN, EDOM and FE_INVALID", tgamma(-one), NAN, EDOM, FE_INVALID);
    return PASS;
}

static const char *erf_erfc(void) {
    EXPECT_DBL("erf(0)", erf(zero), 0.0);
    EXPECT_DBL("erf(-0)", erf(nzero), -0.0);
    EXPECT_DBL("erf(inf)", erf(inf), 1.0);
    EXPECT_DBL("erfc(inf)", erfc(inf), 0.0);
    EXPECT_DBL("erfc(-inf)", erfc(-inf), 2.0);
    EXPECT_NEAR("erf(1)", erf(one), 0.8427007929497149, 2e-16);
    return PASS;
}

static const char *fma_rounding(void) {
    volatile double largest = DBL_MAX;
    volatile float largestf = FLT_MAX;
    errno = 0;
    feclearexcept(FE_ALL_EXCEPT);
    EXPECT_DBL("fma(DBL_MAX,2,-inf)", fma(largest,two,-inf), -INFINITY);
    EXPECT_DBL("fmaf(FLT_MAX,2,-inf)", fmaf(largestf,2.0f,-INFINITY), -INFINITY);
    EXPECT_ERRNO("infinite fma addend", 0);
    CHECK(!fetestexcept(FE_INVALID), "infinite fma addend raised invalid");
    MATH_ERR("fma invalid product errno", fma(zero,inf,one), NAN, EDOM, FE_INVALID);
    MATH_ERR("fma overflow errno", fma(largest,two,zero), INFINITY, ERANGE, FE_OVERFLOW);
    volatile double three = 3.0, four = 4.0, a = 1.0 + 0x1p-52, b = 1.0 - 0x1p-52;
    EXPECT_DBL("fma(2, 3, 4)", fma(two, three, four), 10.0);
    EXPECT_DBL("fma(1+2^-52, 1-2^-52, -1) rounded once", fma(a, b, -one), -0x1p-104);
    return PASS;
}

static const char *fmax_fmin_fdim(void) {
    errno = 0;
    EXPECT_DBL("fdim(inf,0)", fdim(inf,zero), INFINITY);
    EXPECT_ERRNO("fdim(inf,0) has no range error", 0);
    fesetround(FE_DOWNWARD);
    MATH_ERR("fdim downward overflow", fdim(DBL_MAX * one,-DBL_MAX * one), DBL_MAX, ERANGE, FE_OVERFLOW);
    fesetround(FE_TONEAREST);
    volatile double nan = NAN, five = 5.0, three = 3.0;
    EXPECT_DBL("fmax(NaN, 1)", fmax(nan, one), 1.0);
    EXPECT_DBL("fmin(1, NaN)", fmin(one, nan), 1.0);
    EXPECT_DBL("fmax(-1, 2)", fmax(-one, two), 2.0);
    EXPECT_DBL("fdim(5, 3)", fdim(five, three), 2.0);
    EXPECT_DBL("fdim(3, 5)", fdim(three, five), 0.0);
    return PASS;
}

static const char *nextafter_steps(void) {
    volatile double max = DBL_MAX;
    EXPECT_DBL("nextafter(1, 2)", nextafter(one, two), 1.0 + DBL_EPSILON);
    EXPECT_DBL("nextafter(0, 1)", nextafter(zero, one), 0x1p-1074);
    EXPECT_DBL("nextafter(1, 1)", nextafter(one, one), 1.0);
    EXPECT_DBL("nextafter(1, 0)", nextafter(one, zero), 1.0 - DBL_EPSILON / 2);
    MATH_ERR("nextafter(DBL_MAX, inf): inf, ERANGE and FE_OVERFLOW", nextafter(max, inf), INFINITY, ERANGE, FE_OVERFLOW);
    return PASS;
}

static const char *ilogb_logb(void) {
    volatile double eight = 8.0, q = 0.25;
    EXPECT_INT("ilogb(8)", ilogb(eight), 3);
    EXPECT_INT("ilogb(inf)", ilogb(inf), INT_MAX);
    EXPECT_DBL("logb(0.25)", logb(q), -2.0);
    EXPECT_DBL("scalbn(1, 3)", scalbn(one, 3), 8.0);
    EXPECT_DBL("scalbln(1, -2)", scalbln(one, -2L), 0.25);
    feclearexcept(FE_ALL_EXCEPT);
    EXPECT_DBL("logb(0)", logb(zero), -HUGE_VAL);
    CHECK(fetestexcept(FE_DIVBYZERO), "logb(0) did not raise FE_DIVBYZERO");
    feclearexcept(FE_ALL_EXCEPT);
    EXPECT_INT("ilogb(0)", ilogb(zero), FP_ILOGB0);
    CHECK(fetestexcept(FE_INVALID), "ilogb(0) did not raise FE_INVALID");
    return PASS;
}

const struct libc_case libc_cases[] = {
    { "sqrt-values", sqrt_values },
    { "sqrt-domain", sqrt_domain },
    { "errhandling", errhandling },
    { "log-errors", log_errors },
    { "exp-range", exp_range },
    { "pow-special", pow_special },
    { "pow-errors", pow_errors },
    { "fmod-values", fmod_values },
    { "fmod-domain", fmod_domain },
    { "remainder-ties", remainder_ties },
    { "frexp-split", frexp_split },
    { "ldexp-scale", ldexp_scale },
    { "modf-parts", modf_parts },
    { "round-functions", round_functions },
    { "rint-modes", rint_modes },
    { "fesetround-query", fesetround_query },
    { "lround-lrint", lround_lrint },
    { "fe-exceptions", fe_exceptions },
    { "feholdexcept", feholdexcept_env },
    { "fpclassify-kinds", fpclassify_kinds },
    { "nan-quiet-compare", nan_quiet_compare },
    { "copysign-signbit", copysign_signbit },
    { "hypot-no-overflow", hypot_no_overflow },
    { "trig-zeros", trig_zeros },
    { "trig-values", trig_values },
    { "trig-domain", trig_domain },
    { "exp-log-values", exp_log_values },
    { "log1p-expm1", log1p_expm1 },
    { "float-functions", float_functions },
    { "gamma-values", gamma_values },
    { "erf-erfc", erf_erfc },
    { "fma-rounding", fma_rounding },
    { "fmax-fmin-fdim", fmax_fmin_fdim },
    { "nextafter-steps", nextafter_steps },
    { "ilogb-logb", ilogb_logb },
    { 0, 0 },
};
