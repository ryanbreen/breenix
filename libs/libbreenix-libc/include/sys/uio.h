/* sys/uio.h: vector I/O. */
#ifndef _SYS_UIO_H
#define _SYS_UIO_H

#include <__breenix/types.h>
#include <__breenix/iovec.h>
#define __need_size_t
#include <stddef.h>

typedef __breenix_ssize_t ssize_t;

#define UIO_MAXIOV 1024

__BREENIX_BEGIN_DECLS

ssize_t readv(int, const struct iovec *, int);
ssize_t writev(int, const struct iovec *, int);

#ifdef __BREENIX_EXT
ssize_t preadv(int, const struct iovec *, int, __breenix_off_t);
ssize_t pwritev(int, const struct iovec *, int, __breenix_off_t);
#endif

__BREENIX_END_DECLS

#endif
