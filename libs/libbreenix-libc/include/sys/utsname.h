/* sys/utsname.h: system name, in Linux's layout (six 65-byte fields). */
#ifndef _SYS_UTSNAME_H
#define _SYS_UTSNAME_H

#include <__breenix/features.h>

struct utsname {
    char sysname[65];
    char nodename[65];
    char release[65];
    char version[65];
    char machine[65];
#ifdef __BREENIX_EXT
    char domainname[65];
#else
    char __domainname[65];
#endif
};

__BREENIX_BEGIN_DECLS

int uname(struct utsname *);

__BREENIX_END_DECLS

#endif
