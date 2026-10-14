/* Linux sysinfo ABI on Breenix's two 64-bit targets. */
#ifndef _SYS_SYSINFO_H
#define _SYS_SYSINFO_H
#include <__breenix/types.h>
#define SI_LOAD_SHIFT 16
struct sysinfo {
    long uptime;
    unsigned long loads[3];
    unsigned long totalram, freeram, sharedram, bufferram, totalswap, freeswap;
    unsigned short procs;
    unsigned short pad;
    unsigned long totalhigh, freehigh;
    unsigned int mem_unit;
    char _f[20 - 2 * sizeof(long) - sizeof(int)];
};
__BREENIX_BEGIN_DECLS
int sysinfo(struct sysinfo *);
__BREENIX_END_DECLS
#endif
