/* POSIX.1-2024 <dlfcn.h>. The dynamic functions use the hosted runtime. */
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

__BREENIX_BEGIN_DECLS
void *dlopen(const char *, int);
void *dlsym(void *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT);
char *dlerror(void);
int dlclose(void *);
__BREENIX_END_DECLS
#endif /* __BREENIX_POSIX */
#endif
