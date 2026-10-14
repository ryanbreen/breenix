/* struct iovec, shared by sys/uio.h and sys/socket.h. */
#ifndef __BREENIX_IOVEC_H
#define __BREENIX_IOVEC_H
struct iovec {
    void *iov_base;
    __SIZE_TYPE__ iov_len;
};
#endif
