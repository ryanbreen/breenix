/* <wordexp.h>: POSIX.1-2024 word expansion. libc does not define this header's functions yet
 * (issue #1602), so it declares only the types and constants. */
#ifndef __BREENIX_WORDEXP_H
#define __BREENIX_WORDEXP_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#define __need_size_t
#include <stddef.h>

typedef struct {
    size_t we_wordc;
    char **we_wordv;
    size_t we_offs;
} wordexp_t;

#define WRDE_APPEND 0x01
#define WRDE_DOOFFS 0x02
#define WRDE_NOCMD 0x04
#define WRDE_REUSE 0x08
#define WRDE_SHOWERR 0x10
#define WRDE_UNDEF 0x20

#define WRDE_BADCHAR 1
#define WRDE_BADVAL 2
#define WRDE_CMDSUB 3
#define WRDE_NOSPACE 4
#define WRDE_SYNTAX 5
#endif /* __BREENIX_POSIX */
#endif
