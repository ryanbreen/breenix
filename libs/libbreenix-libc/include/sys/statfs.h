/* Breenix filesystem-statistics extension. Fields and spare words match
 * kernel/src/syscall/fs.rs Statfs exactly; numbers/layout are not libc-derived. */
#ifndef _SYS_STATFS_H
#define _SYS_STATFS_H

#include <__breenix/types.h>

typedef struct {
    int __val[2];
} fsid_t;

/* The kernel's layout: fifteen 64-bit words on both architectures. */
struct statfs {
    long f_type;
    long f_bsize;
    __breenix_fsblkcnt_t f_blocks;
    __breenix_fsblkcnt_t f_bfree;
    __breenix_fsblkcnt_t f_bavail;
    __breenix_fsfilcnt_t f_files;
    __breenix_fsfilcnt_t f_ffree;
    fsid_t f_fsid;
    long f_namelen;
    long f_frsize;
    long f_flags;
    long f_spare[4];
};

__BREENIX_BEGIN_DECLS

int statfs(const char *, struct statfs *);
int fstatfs(int, struct statfs *);

#ifdef __BREENIX_EXT
#define statfs64 statfs
#define fstatfs64 fstatfs
#endif

__BREENIX_END_DECLS

#endif
