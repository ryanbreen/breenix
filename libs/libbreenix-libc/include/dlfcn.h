/* <dlfcn.h>: POSIX.1-2024 dynamic linking. libc does not define this header's functions yet
 * (issue #1591), so it declares only the types and constants. */
#ifndef __BREENIX_DLFCN_H
#define __BREENIX_DLFCN_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
typedef struct {
    const char *dli_fname;
    void *dli_fbase;
    const char *dli_sname;
    void *dli_saddr;
} Dl_info_t;

#define RTLD_LAZY 1
#define RTLD_NOW 2
#define RTLD_GLOBAL 0x100
#define RTLD_LOCAL 0
#endif /* __BREENIX_POSIX */
#endif
