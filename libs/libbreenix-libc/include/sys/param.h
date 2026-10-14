/* Breenix system parameter aliases; limits come from this implementation in limits.h. */
#ifndef _SYS_PARAM_H
#define _SYS_PARAM_H

#include <limits.h>
#include <endian.h>
#include <sys/types.h>

#define MAXPATHLEN PATH_MAX
#define MAXHOSTNAMELEN HOST_NAME_MAX
#define MAXSYMLINKS SYMLOOP_MAX
#define NOFILE OPEN_MAX
#define NGROUPS NGROUPS_MAX
#define NBBY CHAR_BIT
/* Kernel CPU-time accounting uses 100 ticks/s (sys_times); device blocks
 * are 512-byte sectors (kernel/src/block); HZ is rusage.rs CLK_TCK. */
#define HZ 100
#define DEV_BSIZE 512

#define MIN(a, b) ((a) < (b) ? (a) : (b))
#define MAX(a, b) ((a) > (b) ? (a) : (b))
#define howmany(x, y) (((x) + ((y) - 1)) / (y))
#define roundup(x, y) ((((x) + ((y) - 1)) / (y)) * (y))
#define powerof2(x) ((((x) - 1) & (x)) == 0)

#endif
