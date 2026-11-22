/* Inject one interruption and one partial read into a large archive link. */
#define _POSIX_C_SOURCE 200809L
#undef read
#include <errno.h>
#include <stdio.h>
#include <unistd.h>

ssize_t bld_test_read(int fd, void *buffer, size_t size)
{
    static unsigned calls;
    ++calls;
    if (calls == 1 && size >= 65536) {
        fputs("read EINTR\n", stderr);
        errno = EINTR;
        return -1;
    }
    if (calls == 2 && size >= 65536) {
        fputs("read short\n", stderr);
        size /= 2;
    } else {
        fputs("read\n", stderr);
    }
    return read(fd, buffer, size);
}
