/* sys/statvfs.h: file system information. */
#ifndef _SYS_STATVFS_H
#define _SYS_STATVFS_H

#include <__breenix/types.h>

typedef __breenix_fsblkcnt_t fsblkcnt_t;
typedef __breenix_fsfilcnt_t fsfilcnt_t;

struct statvfs {
    unsigned long f_bsize;
    unsigned long f_frsize;
    fsblkcnt_t f_blocks;
    fsblkcnt_t f_bfree;
    fsblkcnt_t f_bavail;
    fsfilcnt_t f_files;
    fsfilcnt_t f_ffree;
    fsfilcnt_t f_favail;
    unsigned long f_fsid;
    unsigned long f_flag;
    unsigned long f_namemax;
    int __reserved[6];
};

#define ST_RDONLY 1
#define ST_NOSUID 2

__BREENIX_BEGIN_DECLS

int fstatvfs(int, struct statvfs *);
int statvfs(const char *__BREENIX_RESTRICT, struct statvfs *__BREENIX_RESTRICT);

__BREENIX_END_DECLS

#endif
