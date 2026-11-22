/* N1570 §7.20. AMD64 psABI §3.1.2 and AAPCS64 §§5.1, 10.1 (LP64). */
#ifndef BREENIX_TOOLCHAIN_STDINT_H
#define BREENIX_TOOLCHAIN_STDINT_H

/* These bounds come from the C support headers in this directory. */
#include "limits.h"

typedef signed char int8_t;
typedef unsigned char uint8_t;
typedef short int16_t;
typedef unsigned short uint16_t;
typedef int int32_t;
typedef unsigned int uint32_t;
typedef long int64_t;
typedef unsigned long uint64_t;

typedef int8_t int_least8_t;
typedef uint8_t uint_least8_t;
typedef int16_t int_least16_t;
typedef uint16_t uint_least16_t;
typedef int32_t int_least32_t;
typedef uint32_t uint_least32_t;
typedef int64_t int_least64_t;
typedef uint64_t uint_least64_t;

typedef int int_fast8_t;
typedef unsigned int uint_fast8_t;
typedef int int_fast16_t;
typedef unsigned int uint_fast16_t;
typedef int int_fast32_t;
typedef unsigned int uint_fast32_t;
typedef long int_fast64_t;
typedef unsigned long uint_fast64_t;

typedef long intptr_t;
typedef unsigned long uintptr_t;
typedef long intmax_t;
typedef unsigned long uintmax_t;

#define INT8_MIN SCHAR_MIN
#define INT8_MAX SCHAR_MAX
#define UINT8_MAX UCHAR_MAX
#define INT16_MIN SHRT_MIN
#define INT16_MAX SHRT_MAX
#define UINT16_MAX USHRT_MAX
#define INT32_MIN INT_MIN
#define INT32_MAX INT_MAX
#define UINT32_MAX UINT_MAX
#define INT64_MIN LONG_MIN
#define INT64_MAX LONG_MAX
#define UINT64_MAX ULONG_MAX

#define INT_LEAST8_MIN INT8_MIN
#define INT_LEAST8_MAX INT8_MAX
#define UINT_LEAST8_MAX UINT8_MAX
#define INT_LEAST16_MIN INT16_MIN
#define INT_LEAST16_MAX INT16_MAX
#define UINT_LEAST16_MAX UINT16_MAX
#define INT_LEAST32_MIN INT32_MIN
#define INT_LEAST32_MAX INT32_MAX
#define UINT_LEAST32_MAX UINT32_MAX
#define INT_LEAST64_MIN INT64_MIN
#define INT_LEAST64_MAX INT64_MAX
#define UINT_LEAST64_MAX UINT64_MAX

#define INT_FAST8_MIN INT32_MIN
#define INT_FAST8_MAX INT32_MAX
#define UINT_FAST8_MAX UINT32_MAX
#define INT_FAST16_MIN INT32_MIN
#define INT_FAST16_MAX INT32_MAX
#define UINT_FAST16_MAX UINT32_MAX
#define INT_FAST32_MIN INT32_MIN
#define INT_FAST32_MAX INT32_MAX
#define UINT_FAST32_MAX UINT32_MAX
#define INT_FAST64_MIN INT64_MIN
#define INT_FAST64_MAX INT64_MAX
#define UINT_FAST64_MAX UINT64_MAX

#define INTPTR_MIN INT64_MIN
#define INTPTR_MAX INT64_MAX
#define UINTPTR_MAX UINT64_MAX
#define INTMAX_MIN INT64_MIN
#define INTMAX_MAX INT64_MAX
#define UINTMAX_MAX UINT64_MAX

#define PTRDIFF_MIN INT64_MIN
#define PTRDIFF_MAX INT64_MAX
#define SIZE_MAX UINT64_MAX
#define SIG_ATOMIC_MIN INT_MIN
#define SIG_ATOMIC_MAX INT_MAX

#define WCHAR_MAX __WCHAR_MAX__
#ifdef __WCHAR_UNSIGNED__
#define WCHAR_MIN 0U
#else
#define WCHAR_MIN (-WCHAR_MAX - 1)
#endif

#define WINT_MAX __WINT_MAX__
#ifdef __WINT_UNSIGNED__
#define WINT_MIN 0U
#else
#define WINT_MIN (-WINT_MAX - 1)
#endif

/* Suffixes follow the promoted least-width types in §7.20.4. */
#define __BREENIX_LITERAL_TOKEN(value, suffix) value ## suffix
#define __BREENIX_LITERAL(value, suffix) __BREENIX_LITERAL_TOKEN(value, suffix)

#define INT8_C(value) value
#define UINT8_C(value) value
#define INT16_C(value) value
#define UINT16_C(value) value
#define INT32_C(value) value
#define UINT32_C(value) __BREENIX_LITERAL(value, U)
#define INT64_C(value) __BREENIX_LITERAL(value, L)
#define UINT64_C(value) __BREENIX_LITERAL(value, UL)
#define INTMAX_C(value) INT64_C(value)
#define UINTMAX_C(value) UINT64_C(value)

#endif
