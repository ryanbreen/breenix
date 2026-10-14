#ifndef __BREENIX_SEMAPHORE_H
#define __BREENIX_SEMAPHORE_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
#include <sys/types.h>
typedef __breenix_sem_t sem_t;
#include <__breenix/timespec.h>
#define SEM_FAILED ((sem_t *)-1)
#define SEM_VALUE_MAX 2147483647
#ifdef __cplusplus
extern "C" {
#endif
int sem_init(sem_t *, int, unsigned int);
int sem_destroy(sem_t *);
int sem_wait(sem_t *);
int sem_trywait(sem_t *);
int sem_timedwait(sem_t *, const struct timespec *);
int sem_post(sem_t *);
int sem_getvalue(sem_t *, int *);
sem_t *sem_open(const char *, int, ...);
int sem_close(sem_t *);
int sem_unlink(const char *);
#ifdef __cplusplus
}
#endif
#endif /* __BREENIX_POSIX */
#endif
