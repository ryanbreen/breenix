/* setjmp.h: non-local jumps. */
#ifndef _SETJMP_H
#define _SETJMP_H

#include <__breenix/features.h>

/* The callee-saved registers, stack pointer and return address setjmp records
 * (src/setjmp.rs), then whether sigsetjmp saved the signal mask, and that mask. */
typedef struct __breenix_jmp_buf {
    unsigned long __regs[22];
    unsigned long __mask_saved;
    unsigned long __mask;
} jmp_buf[1];

__BREENIX_BEGIN_DECLS

int setjmp(jmp_buf) __attribute__((__returns_twice__));
__BREENIX_NORETURN void longjmp(jmp_buf, int);

#if defined(__BREENIX_POSIX)
typedef jmp_buf sigjmp_buf;
int sigsetjmp(sigjmp_buf, int) __attribute__((__returns_twice__));
__BREENIX_NORETURN void siglongjmp(sigjmp_buf, int);
int _setjmp(jmp_buf) __attribute__((__returns_twice__));
__BREENIX_NORETURN void _longjmp(jmp_buf, int);
#endif

__BREENIX_END_DECLS

#endif
