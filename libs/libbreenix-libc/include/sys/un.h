/* sys/un.h: UNIX domain socket addresses. */
#ifndef _SYS_UN_H
#define _SYS_UN_H

#include <__breenix/types.h>

typedef __breenix_sa_family_t sa_family_t;

struct sockaddr_un {
    sa_family_t sun_family;
    char sun_path[108];
};

#ifdef __BREENIX_EXT
#define SUN_LEN(s) (__builtin_offsetof(struct sockaddr_un, sun_path) + __builtin_strlen((s)->sun_path))
#endif

#endif
