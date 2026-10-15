/* <sys/ipc.h>: POSIX.1-2024 XSI interprocess communication. struct ipc_perm is the
 * kernel's (Linux's 64-bit ipc64_perm): it begins every msqid_ds, semid_ds and shmid_ds. */
#ifndef __BREENIX_SYS_IPC_H
#define __BREENIX_SYS_IPC_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
typedef __breenix_uid_t uid_t;
typedef __breenix_gid_t gid_t;
typedef __breenix_mode_t mode_t;
typedef __breenix_key_t key_t;

struct ipc_perm {
    key_t __key;
    uid_t uid;
    gid_t gid;
    uid_t cuid;
    gid_t cgid;
    mode_t mode;
    unsigned short __seq;
    unsigned short __pad1;
    unsigned int __pad2;
    unsigned long __unused[2];
};

#define IPC_CREAT 01000
#define IPC_EXCL 02000
#define IPC_NOWAIT 04000
#define IPC_PRIVATE ((key_t)0)
#define IPC_RMID 0
#define IPC_SET 1
#define IPC_STAT 2
#ifdef __BREENIX_EXT
#define IPC_INFO 3
#endif

__BREENIX_BEGIN_DECLS
key_t ftok(const char *, int);
__BREENIX_END_DECLS
#endif /* __BREENIX_POSIX */
#endif
