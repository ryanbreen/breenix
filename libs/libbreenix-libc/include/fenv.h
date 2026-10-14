/* <fenv.h>: ISO C17 7.6. The values are Linux's for each architecture, which are the
 * hardware's own bit positions: FPSR flags and FPCR.RMode on aarch64; the x87 status
 * and control words (mirrored in MXCSR) on x86-64. */
#ifndef __BREENIX_FENV_H
#define __BREENIX_FENV_H

#include <__breenix/features.h>

#if defined(__aarch64__)
#define FE_INVALID 1
#define FE_DIVBYZERO 2
#define FE_OVERFLOW 4
#define FE_UNDERFLOW 8
#define FE_INEXACT 16
#define FE_ALL_EXCEPT 31
#define FE_TONEAREST 0
#define FE_UPWARD 0x400000
#define FE_DOWNWARD 0x800000
#define FE_TOWARDZERO 0xc00000
typedef unsigned int fexcept_t;
typedef struct {
    unsigned int __fpcr;
    unsigned int __fpsr;
} fenv_t;
#elif defined(__x86_64__)
#define FE_INVALID 1
#define FE_DIVBYZERO 4
#define FE_OVERFLOW 8
#define FE_UNDERFLOW 0x10
#define FE_INEXACT 0x20
#define FE_ALL_EXCEPT 0x3d
#define FE_TONEAREST 0
#define FE_DOWNWARD 0x400
#define FE_UPWARD 0x800
#define FE_TOWARDZERO 0xc00
typedef unsigned short fexcept_t;
/* The 28-byte x87 environment image (fnstenv), then MXCSR. */
typedef struct {
    unsigned short __x87[14];
    unsigned int __mxcsr;
} fenv_t;
#else
#error "fenv.h: unsupported architecture"
#endif

#define FE_DFL_ENV ((const fenv_t *)-1)

__BREENIX_BEGIN_DECLS

int feclearexcept(int excepts);
int fegetexceptflag(fexcept_t *flagp, int excepts);
int feraiseexcept(int excepts);
int fesetexceptflag(const fexcept_t *flagp, int excepts);
int fetestexcept(int excepts);
int fegetround(void);
int fesetround(int round);
int fegetenv(fenv_t *envp);
int feholdexcept(fenv_t *envp);
int fesetenv(const fenv_t *envp);
int feupdateenv(const fenv_t *envp);

__BREENIX_END_DECLS

#endif
