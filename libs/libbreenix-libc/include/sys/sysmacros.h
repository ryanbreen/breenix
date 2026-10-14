/* Device encoding is derived from kernel/src/fs/ext2/create_node.rs: a
 * 12-bit major in bits 8..19, an 8-bit low minor in bits 0..7, and the remaining
 * 12 minor bits in bits 20..31. The kernel stores device numbers in 32 bits. */
#ifndef _SYS_SYSMACROS_H
#define _SYS_SYSMACROS_H

#define major(d) ((unsigned)(((unsigned long long)(d) >> 8) & 0xfffu))
#define minor(d) ((unsigned)(((unsigned long long)(d) & 0xffu) | \
                             (((unsigned long long)(d) >> 12) & 0xfff00u)))
#define makedev(ma, mi) ((((unsigned long long)(ma) & 0xfffull) << 8) | \
                         ((unsigned long long)(mi) & 0xffull) | \
                         (((unsigned long long)(mi) & 0xfff00ull) << 12))

#endif
