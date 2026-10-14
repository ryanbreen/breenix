/* sys/select.h: synchronous I/O multiplexing. */
#ifndef _SYS_SELECT_H
#define _SYS_SELECT_H

#include <__breenix/types.h>
#include <__breenix/timespec.h>
#include <__breenix/timeval.h>

typedef __breenix_time_t time_t;
typedef __breenix_suseconds_t suseconds_t;
typedef __breenix_sigset_t sigset_t;

#define FD_SETSIZE 1024

typedef struct {
    unsigned long fds_bits[FD_SETSIZE / (8 * sizeof(unsigned long))];
} fd_set;

#define __BREENIX_FD_WORD(fd) ((unsigned)(fd) / (8 * sizeof(unsigned long)))
#define __BREENIX_FD_BIT(fd) (1ul << ((unsigned)(fd) % (8 * sizeof(unsigned long))))
#define FD_SET(fd, set) ((void)((set)->fds_bits[__BREENIX_FD_WORD(fd)] |= __BREENIX_FD_BIT(fd)))
#define FD_CLR(fd, set) ((void)((set)->fds_bits[__BREENIX_FD_WORD(fd)] &= ~__BREENIX_FD_BIT(fd)))
#define FD_ISSET(fd, set) (((set)->fds_bits[__BREENIX_FD_WORD(fd)] & __BREENIX_FD_BIT(fd)) != 0)
#define FD_ZERO(set) do { \
        for (unsigned __breenix_i = 0; __breenix_i < sizeof((set)->fds_bits) / sizeof((set)->fds_bits[0]); \
             __breenix_i++) \
            (set)->fds_bits[__breenix_i] = 0; \
    } while (0)

__BREENIX_BEGIN_DECLS

int pselect(int, fd_set *__BREENIX_RESTRICT, fd_set *__BREENIX_RESTRICT, fd_set *__BREENIX_RESTRICT,
            const struct timespec *__BREENIX_RESTRICT, const sigset_t *__BREENIX_RESTRICT);
int select(int, fd_set *__BREENIX_RESTRICT, fd_set *__BREENIX_RESTRICT, fd_set *__BREENIX_RESTRICT,
           struct timeval *__BREENIX_RESTRICT);

__BREENIX_END_DECLS

#endif
