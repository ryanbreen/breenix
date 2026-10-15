/* <math.h>: ISO C17 7.12 with Annex F, and POSIX.1-2024 with XSI. long double is IEEE
 * binary128 on aarch64 and the x87 80-bit format on x86_64. Every domain, pole and range
 * error sets errno and raises its exception. */
#ifndef __BREENIX_MATH_H
#define __BREENIX_MATH_H

#include <__breenix/features.h>

#if __FLT_EVAL_METHOD__ == 0
typedef float float_t;
typedef double double_t;
#elif __FLT_EVAL_METHOD__ == 1
typedef double float_t;
typedef double double_t;
#else
typedef long double float_t;
typedef long double double_t;
#endif

#define HUGE_VAL (__builtin_huge_val())
#define HUGE_VALF (__builtin_huge_valf())
#define HUGE_VALL (__builtin_huge_vall())
#define INFINITY (__builtin_inff())
#define NAN (__builtin_nanf(""))

#define FP_NAN 0
#define FP_INFINITE 1
#define FP_ZERO 2
#define FP_SUBNORMAL 3
#define FP_NORMAL 4

#if defined(__x86_64__)
#define FP_ILOGB0 (-2147483647 - 1)
#define FP_ILOGBNAN (-2147483647 - 1)
#else
#define FP_ILOGB0 (-2147483647)
#define FP_ILOGBNAN 2147483647
#endif

#if defined(__aarch64__)
#define FP_FAST_FMA 1
#define FP_FAST_FMAF 1
#endif

#define MATH_ERRNO 1
#define MATH_ERREXCEPT 2
#define math_errhandling (MATH_ERRNO | MATH_ERREXCEPT)

#define fpclassify(x) __builtin_fpclassify(FP_NAN, FP_INFINITE, FP_NORMAL, FP_SUBNORMAL, FP_ZERO, (x))
#define isfinite(x) __builtin_isfinite(x)
#define isinf(x) __builtin_isinf(x)
#define isnan(x) __builtin_isnan(x)
#define isnormal(x) __builtin_isnormal(x)
#define signbit(x) __builtin_signbit(x)
#define isgreater(x, y) __builtin_isgreater((x), (y))
#define isgreaterequal(x, y) __builtin_isgreaterequal((x), (y))
#define isless(x, y) __builtin_isless((x), (y))
#define islessequal(x, y) __builtin_islessequal((x), (y))
#define islessgreater(x, y) __builtin_islessgreater((x), (y))
#define isunordered(x, y) __builtin_isunordered((x), (y))

#ifdef __BREENIX_POSIX
#define M_E 2.7182818284590452354
#define M_LOG2E 1.4426950408889634074
#define M_LOG10E 0.43429448190325182765
#define M_LN2 0.69314718055994530942
#define M_LN10 2.30258509299404568402
#define M_PI 3.14159265358979323846
#define M_PI_2 1.57079632679489661923
#define M_PI_4 0.78539816339744830962
#define M_1_PI 0.31830988618379067154
#define M_2_PI 0.63661977236758134308
#define M_2_SQRTPI 1.12837916709551257390
#define M_SQRT2 1.41421356237309504880
#define M_SQRT1_2 0.70710678118654752440
#endif

__BREENIX_BEGIN_DECLS

double acos(double x);
double asin(double x);
double atan(double x);
double cos(double x);
double sin(double x);
double tan(double x);
double acosh(double x);
double asinh(double x);
double atanh(double x);
double cosh(double x);
double sinh(double x);
double tanh(double x);
double exp(double x);
double exp2(double x);
double expm1(double x);
double log(double x);
double log10(double x);
double log1p(double x);
double log2(double x);
double logb(double x);
double cbrt(double x);
double fabs(double x);
double sqrt(double x);
double erf(double x);
double erfc(double x);
double lgamma(double x);
double tgamma(double x);
double ceil(double x);
double floor(double x);
double nearbyint(double x);
double rint(double x);
double round(double x);
double trunc(double x);
double atan2(double x, double y);
double hypot(double x, double y);
double pow(double x, double y);
double fmod(double x, double y);
double remainder(double x, double y);
double copysign(double x, double y);
double nextafter(double x, double y);
double fdim(double x, double y);
double fmax(double x, double y);
double fmin(double x, double y);
double fma(double x, double y, double z);
double frexp(double x, int *exp);
double ldexp(double x, int exp);
double scalbn(double x, int n);
double scalbln(double x, long n);
int ilogb(double x);
double modf(double x, double *iptr);
double remquo(double x, double y, int *quo);
double nan(const char *tag);
long lrint(double x);
long lround(double x);
long long llrint(double x);
long long llround(double x);

float acosf(float x);
float asinf(float x);
float atanf(float x);
float cosf(float x);
float sinf(float x);
float tanf(float x);
float acoshf(float x);
float asinhf(float x);
float atanhf(float x);
float coshf(float x);
float sinhf(float x);
float tanhf(float x);
float expf(float x);
float exp2f(float x);
float expm1f(float x);
float logf(float x);
float log10f(float x);
float log1pf(float x);
float log2f(float x);
float logbf(float x);
float cbrtf(float x);
float fabsf(float x);
float sqrtf(float x);
float erff(float x);
float erfcf(float x);
float lgammaf(float x);
float tgammaf(float x);
float ceilf(float x);
float floorf(float x);
float nearbyintf(float x);
float rintf(float x);
float roundf(float x);
float truncf(float x);
float atan2f(float x, float y);
float hypotf(float x, float y);
float powf(float x, float y);
float fmodf(float x, float y);
float remainderf(float x, float y);
float copysignf(float x, float y);
float nextafterf(float x, float y);
float fdimf(float x, float y);
float fmaxf(float x, float y);
float fminf(float x, float y);
float fmaf(float x, float y, float z);
float frexpf(float x, int *exp);
float ldexpf(float x, int exp);
float scalbnf(float x, int n);
float scalblnf(float x, long n);
int ilogbf(float x);
float modff(float x, float *iptr);
float remquof(float x, float y, int *quo);
float nanf(const char *tag);
long lrintf(float x);
long lroundf(float x);
long long llrintf(float x);
long long llroundf(float x);

long double acosl(long double x);
long double asinl(long double x);
long double atanl(long double x);
long double cosl(long double x);
long double sinl(long double x);
long double tanl(long double x);
long double acoshl(long double x);
long double asinhl(long double x);
long double atanhl(long double x);
long double coshl(long double x);
long double sinhl(long double x);
long double tanhl(long double x);
long double expl(long double x);
long double exp2l(long double x);
long double expm1l(long double x);
long double logl(long double x);
long double log10l(long double x);
long double log1pl(long double x);
long double log2l(long double x);
long double logbl(long double x);
long double cbrtl(long double x);
long double fabsl(long double x);
long double sqrtl(long double x);
long double erfl(long double x);
long double erfcl(long double x);
long double lgammal(long double x);
long double tgammal(long double x);
long double ceill(long double x);
long double floorl(long double x);
long double nearbyintl(long double x);
long double rintl(long double x);
long double roundl(long double x);
long double truncl(long double x);
long double atan2l(long double x, long double y);
long double hypotl(long double x, long double y);
long double powl(long double x, long double y);
long double fmodl(long double x, long double y);
long double remainderl(long double x, long double y);
long double copysignl(long double x, long double y);
long double nextafterl(long double x, long double y);
long double fdiml(long double x, long double y);
long double fmaxl(long double x, long double y);
long double fminl(long double x, long double y);
long double fmal(long double x, long double y, long double z);
long double frexpl(long double x, int *exp);
long double ldexpl(long double x, int exp);
long double scalbnl(long double x, int n);
long double scalblnl(long double x, long n);
int ilogbl(long double x);
long double modfl(long double x, long double *iptr);
long double remquol(long double x, long double y, int *quo);
long double nanl(const char *tag);
long lrintl(long double x);
long lroundl(long double x);
long long llrintl(long double x);
long long llroundl(long double x);

double nexttoward(double x, long double y);
float nexttowardf(float x, long double y);
long double nexttowardl(long double x, long double y);

#ifdef __BREENIX_POSIX
extern int signgam;
#endif

#if defined(__STDC_VERSION__) && __STDC_VERSION__ > 201710L
double roundeven(double x);
float roundevenf(float x);
double fmaximum(double x, double y);
float fmaximumf(float x, float y);
double fminimum(double x, double y);
float fminimumf(float x, float y);
#endif

#ifdef __BREENIX_EXT
double lgamma_r(double x, int *sign);
float lgammaf_r(float x, int *sign);
long double lgammal_r(long double x, int *sign);
#endif

__BREENIX_END_DECLS

#endif
