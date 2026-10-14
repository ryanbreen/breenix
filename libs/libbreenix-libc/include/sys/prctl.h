/* Breenix process-control extension. ISO C17/POSIX.1-2024 do not specify
 * these operations. The implementation currently returns ENOSYS for every one.
 * Request IDs are our reserved "BR" namespace (ASCII B=0x42, R=0x52 in the
 * implementation's execution character set), followed by an ordinal. */
#ifndef _SYS_PRCTL_H
#define _SYS_PRCTL_H

#include <__breenix/features.h>

#define PR_SET_PDEATHSIG 0x42520001
#define PR_GET_PDEATHSIG 0x42520002
#define PR_GET_DUMPABLE 0x42520003
#define PR_SET_DUMPABLE 0x42520004
#define PR_SET_NAME 0x42520005
#define PR_GET_NAME 0x42520006
#define PR_SET_CHILD_SUBREAPER 0x42520007
#define PR_GET_CHILD_SUBREAPER 0x42520008
#define PR_SET_NO_NEW_PRIVS 0x42520009
#define PR_GET_NO_NEW_PRIVS 0x4252000a

__BREENIX_BEGIN_DECLS

int prctl(int, ...);

__BREENIX_END_DECLS

#endif
