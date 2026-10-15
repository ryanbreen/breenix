/* <aio.h>: POSIX.1-2024 asynchronous input and output. libc does not define this header's functions yet
 * (issue #1589), so it declares only the types and constants. */
#ifndef __BREENIX_AIO_H
#define __BREENIX_AIO_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
#include <__breenix/timespec.h>
#include <signal.h>
#define __need_size_t
#include <stddef.h>
typedef __breenix_off_t off_t;
typedef __breenix_ssize_t ssize_t;
typedef __breenix_pthread_attr_t pthread_attr_t;

struct aiocb {
    int aio_fildes;
    off_t aio_offset;
    volatile void *aio_buf;
    size_t aio_nbytes;
    int aio_reqprio;
    struct sigevent aio_sigevent;
    int aio_lio_opcode;
};

#define AIO_CANCELED 0
#define AIO_NOTCANCELED 1
#define AIO_ALLDONE 2
#define LIO_READ 0
#define LIO_WRITE 1
#define LIO_NOP 2
#define LIO_WAIT 0
#define LIO_NOWAIT 1
#endif /* __BREENIX_POSIX */
#endif
