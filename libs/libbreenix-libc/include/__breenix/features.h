/*
 * Which names Breenix's headers declare, and the declaration helpers they share.
 *
 *   -std=c17 (or c99/c11) with no feature macro: ISO C names only.
 *   otherwise (-std=gnu*, or _POSIX_C_SOURCE, _XOPEN_SOURCE and friends):
 *       POSIX.1-2024 with the XSI option.
 *   _GNU_SOURCE, _DEFAULT_SOURCE or _BSD_SOURCE: also the GNU and BSD extensions
 *       Breenix implements.
 *
 * Every header includes this first. Internal names start with __breenix or __BREENIX.
 */
#ifndef __BREENIX_FEATURES_H
#define __BREENIX_FEATURES_H

#if defined(_GNU_SOURCE) || defined(_DEFAULT_SOURCE) || defined(_BSD_SOURCE)
#define __BREENIX_EXT 1
#endif

#if defined(__BREENIX_EXT) || defined(_POSIX_SOURCE) || defined(_POSIX_C_SOURCE) || \
    defined(_XOPEN_SOURCE) || !defined(__STRICT_ANSI__)
#define __BREENIX_POSIX 1
#endif

#ifdef __cplusplus
#define __BREENIX_BEGIN_DECLS extern "C" {
#define __BREENIX_END_DECLS }
#define __BREENIX_RESTRICT __restrict
#define __BREENIX_NORETURN [[noreturn]]
#else
#define __BREENIX_BEGIN_DECLS
#define __BREENIX_END_DECLS
/* C89 and C90 have neither keyword; the compiler's spellings work in every mode. */
#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 199901L
#define __BREENIX_RESTRICT restrict
#else
#define __BREENIX_RESTRICT __restrict
#endif
#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
#define __BREENIX_NORETURN _Noreturn
#else
#define __BREENIX_NORETURN __attribute__((__noreturn__))
#endif
#endif

#endif
