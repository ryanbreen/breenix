/* sys/prctl.h: operations on a process or thread (a Linux header, with Linux's values). */
#ifndef _SYS_PRCTL_H
#define _SYS_PRCTL_H

#include <__breenix/features.h>

#define PR_SET_PDEATHSIG 1
#define PR_GET_PDEATHSIG 2
#define PR_GET_DUMPABLE 3
#define PR_SET_DUMPABLE 4
#define PR_SET_NAME 15
#define PR_GET_NAME 16
#define PR_SET_CHILD_SUBREAPER 36
#define PR_GET_CHILD_SUBREAPER 37
#define PR_SET_NO_NEW_PRIVS 38
#define PR_GET_NO_NEW_PRIVS 39

__BREENIX_BEGIN_DECLS

int prctl(int, ...);

__BREENIX_END_DECLS

#endif
