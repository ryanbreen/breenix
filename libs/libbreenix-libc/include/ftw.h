/* <ftw.h>: POSIX.1-2024 file tree walks. libc does not define this header's functions yet
 * (issue #1593), so it declares only the types and constants. */
#ifndef __BREENIX_FTW_H
#define __BREENIX_FTW_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <sys/stat.h>

struct FTW {
    int base;
    int level;
};

#define FTW_F 0
#define FTW_D 1
#define FTW_DNR 2
#define FTW_DP 3
#define FTW_NS 4
#define FTW_SL 5
#define FTW_SLN 6

#define FTW_PHYS 1
#define FTW_MOUNT 2
#define FTW_DEPTH 4
#define FTW_CHDIR 8
#endif /* __BREENIX_POSIX */
#endif
