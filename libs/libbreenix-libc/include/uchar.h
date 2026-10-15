/* <uchar.h>: ISO C17 7.28 Unicode utilities. char16_t holds UTF-16 code units and
 * char32_t UTF-32 ones; mbrtoc16 returns a character beyond the Basic Multilingual
 * Plane as its two surrogates (src/uchar.rs). */
#ifndef __BREENIX_UCHAR_H
#define __BREENIX_UCHAR_H
#include <__breenix/features.h>
#include <__breenix/types.h>
#define __need_size_t
#include <stddef.h>

typedef __breenix_mbstate_t mbstate_t;
#ifndef __cplusplus
typedef __UINT_LEAST16_TYPE__ char16_t;
typedef __UINT_LEAST32_TYPE__ char32_t;
#endif

__BREENIX_BEGIN_DECLS
size_t c16rtomb(char *__BREENIX_RESTRICT, char16_t, mbstate_t *__BREENIX_RESTRICT);
size_t c32rtomb(char *__BREENIX_RESTRICT, char32_t, mbstate_t *__BREENIX_RESTRICT);
size_t mbrtoc16(char16_t *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, size_t,
                mbstate_t *__BREENIX_RESTRICT);
size_t mbrtoc32(char32_t *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT, size_t,
                mbstate_t *__BREENIX_RESTRICT);
__BREENIX_END_DECLS
#endif
