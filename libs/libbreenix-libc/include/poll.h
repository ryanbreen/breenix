/* poll.h: definitions for poll(), with Linux's event bits. */
#ifndef _POLL_H
#define _POLL_H

#include <__breenix/types.h>

typedef __breenix_nfds_t nfds_t;

struct pollfd {
    int fd;
    short events;
    short revents;
};

#define POLLIN 0x001
#define POLLPRI 0x002
#define POLLOUT 0x004
#define POLLERR 0x008
#define POLLHUP 0x010
#define POLLNVAL 0x020
#define POLLRDNORM 0x040
#define POLLRDBAND 0x080
#define POLLWRNORM 0x100
#define POLLWRBAND 0x200
#ifdef __BREENIX_EXT
#define POLLRDHUP 0x2000
#endif

__BREENIX_BEGIN_DECLS

int poll(struct pollfd[], nfds_t, int);

#ifdef __BREENIX_EXT
#include <__breenix/timespec.h>
int ppoll(struct pollfd *, nfds_t, const struct timespec *, const __breenix_sigset_t *);
#endif

__BREENIX_END_DECLS

#endif
