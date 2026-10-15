/* <sys/msg.h>: POSIX.1-2024 XSI message queues. struct msqid_ds is the kernel's
 * (Linux's 64-bit msqid64_ds, 120 bytes). */
#ifndef __BREENIX_SYS_MSG_H
#define __BREENIX_SYS_MSG_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
#include <sys/ipc.h>
#define __need_size_t
#include <stddef.h>
typedef __breenix_pid_t pid_t;
typedef __breenix_ssize_t ssize_t;
typedef __breenix_time_t time_t;
typedef unsigned long msgqnum_t;
typedef unsigned long msglen_t;

struct msqid_ds {
    struct ipc_perm msg_perm;
    time_t msg_stime;
    time_t msg_rtime;
    time_t msg_ctime;
    unsigned long __msg_cbytes;
    msgqnum_t msg_qnum;
    msglen_t msg_qbytes;
    pid_t msg_lspid;
    pid_t msg_lrpid;
    unsigned long __unused[2];
};

#define MSG_NOERROR 010000
#ifdef __BREENIX_EXT
#define MSG_EXCEPT 020000
#define MSG_STAT 11
#define MSG_INFO 12
#endif

__BREENIX_BEGIN_DECLS
int msgctl(int, int, struct msqid_ds *);
int msgget(key_t, int);
ssize_t msgrcv(int, void *, size_t, long, int);
int msgsnd(int, const void *, size_t, int);
__BREENIX_END_DECLS
#endif /* __BREENIX_POSIX */
#endif
