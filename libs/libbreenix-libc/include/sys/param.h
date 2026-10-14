/* sys/param.h: system parameters (a BSD header). */
#ifndef _SYS_PARAM_H
#define _SYS_PARAM_H

#include <limits.h>
#include <endian.h>
#include <sys/types.h>

#define MAXPATHLEN PATH_MAX
#define MAXHOSTNAMELEN 64
#define MAXSYMLINKS 40
#define NOFILE 1024
#define NGROUPS NGROUPS_MAX
#define NBBY 8
#define HZ 100
#define DEV_BSIZE 512

#define MIN(a, b) ((a) < (b) ? (a) : (b))
#define MAX(a, b) ((a) > (b) ? (a) : (b))
#define howmany(x, y) (((x) + ((y) - 1)) / (y))
#define roundup(x, y) ((((x) + ((y) - 1)) / (y)) * (y))
#define powerof2(x) ((((x) - 1) & (x)) == 0)

#endif
