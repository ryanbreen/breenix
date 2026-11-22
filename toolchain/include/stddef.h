/* N1570 §7.19. AMD64 psABI §3.1.2 and AAPCS64 §§5.1, 10.1 (LP64). */
#ifndef BREENIX_TOOLCHAIN_STDDEF_H
#define BREENIX_TOOLCHAIN_STDDEF_H

typedef long ptrdiff_t;
typedef unsigned long size_t;
typedef __WCHAR_TYPE__ wchar_t;

/* Long double has the largest fundamental alignment in both ELF targets. */
typedef union {
    long double __floating;
    long long __integer;
} max_align_t;

#define NULL ((void *)0)
#define offsetof(type, member) __builtin_offsetof(type, member)

#endif
