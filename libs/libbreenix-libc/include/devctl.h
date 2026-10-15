/* <devctl.h>: POSIX.1-2024 device control. libc does not define this header's functions yet
 * (issue #1590), so it declares only the types and constants. */
#ifndef __BREENIX_DEVCTL_H
#define __BREENIX_DEVCTL_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#define __need_size_t
#include <stddef.h>
#endif /* __BREENIX_POSIX */
#endif
