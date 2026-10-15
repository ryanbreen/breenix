/* <iconv.h>: POSIX.1-2024 codeset conversion. libc does not define this header's functions yet
 * (issue #1594), so it declares only the types and constants. */
#ifndef __BREENIX_ICONV_H
#define __BREENIX_ICONV_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#define __need_size_t
#include <stddef.h>
typedef void *iconv_t;
#endif /* __BREENIX_POSIX */
#endif
