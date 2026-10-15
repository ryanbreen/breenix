/* <mqueue.h>: POSIX.1-2024 message queues. struct mq_attr is the kernel's. */
#ifndef __BREENIX_MQUEUE_H
#define __BREENIX_MQUEUE_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
#include <__breenix/timespec.h>
#include <signal.h>
#define __need_size_t
#include <stddef.h>
typedef __breenix_ssize_t ssize_t;
typedef __breenix_pthread_attr_t pthread_attr_t;
typedef int mqd_t;

struct mq_attr {
    long mq_flags;
    long mq_maxmsg;
    long mq_msgsize;
    long mq_curmsgs;
    long __reserved[4];
};

__BREENIX_BEGIN_DECLS
int mq_close(mqd_t);
int mq_getattr(mqd_t, struct mq_attr *);
int mq_notify(mqd_t, const struct sigevent *);
mqd_t mq_open(const char *, int, ...);
ssize_t mq_receive(mqd_t, char *, size_t, unsigned int *);
int mq_send(mqd_t, const char *, size_t, unsigned int);
int mq_setattr(mqd_t, const struct mq_attr *__BREENIX_RESTRICT, struct mq_attr *__BREENIX_RESTRICT);
ssize_t mq_timedreceive(mqd_t, char *__BREENIX_RESTRICT, size_t, unsigned int *__BREENIX_RESTRICT,
                        const struct timespec *__BREENIX_RESTRICT);
int mq_timedsend(mqd_t, const char *, size_t, unsigned int, const struct timespec *);
int mq_unlink(const char *);
__BREENIX_END_DECLS
#endif /* __BREENIX_POSIX */
#endif
