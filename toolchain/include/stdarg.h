/* N1570 §7.16. AMD64 psABI §3.5.7 and AAPCS64 §§10.1.5, 14. */
#if !defined(__need___va_list) && !defined(__need_va_list)
#ifndef BREENIX_TOOLCHAIN_STDARG_H
#define BREENIX_TOOLCHAIN_STDARG_H

#define __need___va_list
#define __need_va_list

#define va_start(list, last) __builtin_va_start(list, last)
#define va_arg(list, type) __builtin_va_arg(list, type)
#define va_copy(dest, source) __builtin_va_copy(dest, source)
#define va_end(list) __builtin_va_end(list)

#endif
#endif

/* libc's wchar.h can request either typedef without the public macros. */
#ifdef __need___va_list
#ifndef __BREENIX_TOOLCHAIN_GNUC_VA_LIST
#define __BREENIX_TOOLCHAIN_GNUC_VA_LIST
typedef __builtin_va_list __gnuc_va_list;
#endif
#undef __need___va_list
#endif

#ifdef __need_va_list
#ifndef __BREENIX_TOOLCHAIN_VA_LIST
#define __BREENIX_TOOLCHAIN_VA_LIST
typedef __builtin_va_list va_list;
#endif
#undef __need_va_list
#endif
