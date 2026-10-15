/* <sys/shm.h>: POSIX.1-2024 XSI shared memory. struct shmid_ds is the kernel's (Linux's
 * 64-bit shmid64_ds, 112 bytes). */
#ifndef __BREENIX_SYS_SHM_H
#define __BREENIX_SYS_SHM_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
#include <sys/ipc.h>
#define __need_size_t
#include <stddef.h>
typedef __breenix_pid_t pid_t;
typedef __breenix_time_t time_t;
typedef unsigned long shmatt_t;

struct shmid_ds {
    struct ipc_perm shm_perm;
    size_t shm_segsz;
    time_t shm_atime;
    time_t shm_dtime;
    time_t shm_ctime;
    pid_t shm_cpid;
    pid_t shm_lpid;
    shmatt_t shm_nattch;
    unsigned long __unused[2];
};

#define SHM_RDONLY 010000
#define SHM_RND 020000
#define SHMLBA 4096
#ifdef __BREENIX_EXT
#define SHM_EXEC 0100000
#define SHM_REMAP 040000
#define SHM_LOCK 11
#define SHM_UNLOCK 12
#define SHM_STAT 13
#define SHM_INFO 14
#endif

__BREENIX_BEGIN_DECLS
void *shmat(int, const void *, int);
int shmctl(int, int, struct shmid_ds *);
int shmdt(const void *);
int shmget(key_t, size_t, int);
__BREENIX_END_DECLS
#endif /* __BREENIX_POSIX */
#endif
