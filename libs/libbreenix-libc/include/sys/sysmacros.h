/* sys/sysmacros.h: device numbers, in Linux's encoding (12-bit major and 20-bit minor
 * in the low word, the rest of each above). */
#ifndef _SYS_SYSMACROS_H
#define _SYS_SYSMACROS_H

#define major(d) ((unsigned)((((unsigned long long)(d) >> 32) & 0xfffff000u) | \
                             (((unsigned long long)(d) >> 8) & 0xfffu)))
#define minor(d) ((unsigned)((((unsigned long long)(d) >> 12) & 0xffffff00u) | \
                             ((unsigned long long)(d) & 0xffu)))
#define makedev(ma, mi) ((((unsigned long long)(ma) & 0xfffff000ull) << 32) | \
                         (((unsigned long long)(ma) & 0xfffull) << 8) | \
                         (((unsigned long long)(mi) & 0xffffff00ull) << 12) | \
                         ((unsigned long long)(mi) & 0xffull))

#endif
