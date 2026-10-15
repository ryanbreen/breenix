/* <sys/sem.h>: POSIX.1-2024 XSI semaphores. struct semid_ds is the kernel's (Linux's
 * 64-bit semid64_ds): on x86_64 each time is followed by a padding word, and sem_nsems
 * is the low half of a 64-bit field on both little-endian architectures. */
#ifndef __BREENIX_SYS_SEM_H
#define __BREENIX_SYS_SEM_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
#include <__breenix/timespec.h>
#include <sys/ipc.h>
#define __need_size_t
#include <stddef.h>
typedef __breenix_pid_t pid_t;
typedef __breenix_time_t time_t;

struct semid_ds {
    struct ipc_perm sem_perm;
    time_t sem_otime;
#if defined(__x86_64__)
    unsigned long __pad1;
#endif
    time_t sem_ctime;
#if defined(__x86_64__)
    unsigned long __pad2;
#endif
    unsigned short sem_nsems;
    unsigned short __pad3[3];
    unsigned long __unused[2];
};

struct sembuf {
    unsigned short sem_num;
    short sem_op;
    short sem_flg;
};

#define SEM_UNDO 0x1000
#define GETPID 11
#define GETVAL 12
#define GETALL 13
#define GETNCNT 14
#define GETZCNT 15
#define SETVAL 16
#define SETALL 17
#ifdef __BREENIX_EXT
#define SEM_STAT 18
#define SEM_INFO 19
#endif

__BREENIX_BEGIN_DECLS
int semctl(int, int, int, ...);
int semget(key_t, int, int);
int semop(int, struct sembuf *, size_t);
#ifdef __BREENIX_EXT
int semtimedop(int, struct sembuf *, size_t, const struct timespec *);
#endif
__BREENIX_END_DECLS
#endif /* __BREENIX_POSIX */
#endif
