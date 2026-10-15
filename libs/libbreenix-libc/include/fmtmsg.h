/* <fmtmsg.h>: POSIX.1-2024 message display. libc does not define this header's functions yet
 * (issue #1592), so it declares only the types and constants. */
#ifndef __BREENIX_FMTMSG_H
#define __BREENIX_FMTMSG_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#define MM_HARD 0x001
#define MM_SOFT 0x002
#define MM_FIRM 0x004
#define MM_APPL 0x008
#define MM_UTIL 0x010
#define MM_OPSYS 0x020
#define MM_RECOVER 0x040
#define MM_NRECOV 0x080
#define MM_PRINT 0x100
#define MM_CONSOLE 0x200

#define MM_NOSEV 0
#define MM_HALT 1
#define MM_ERROR 2
#define MM_WARNING 3
#define MM_INFO 4

#define MM_NULLLBL ((char *)0)
#define MM_NULLSEV 0
#define MM_NULLMC 0L
#define MM_NULLTXT ((char *)0)
#define MM_NULLACT ((char *)0)
#define MM_NULLTAG ((char *)0)

#define MM_OK 0
#define MM_NOTOK (-1)
#define MM_NOMSG 1
#define MM_NOCON 4
#endif /* __BREENIX_POSIX */
#endif
