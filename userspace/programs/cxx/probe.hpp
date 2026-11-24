#pragma once

#include <errno.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

inline int pipe_fd(int argc, char **argv) {
    if (argc != 2) {
        return -1;
    }
    char *end = nullptr;
    errno = 0;
    const long fd = strtol(argv[1], &end, 10);
    if (errno != 0 || end == argv[1] || *end != '\0' || fd < 0 || fd > INT_MAX) {
        return -1;
    }
    return static_cast<int>(fd);
}

inline bool write_event(int fd, const char *event) {
    for (size_t left = strlen(event); left != 0;) {
        const ssize_t written = write(fd, event, left);
        if (written < 0 && errno == EINTR) {
            continue;
        }
        if (written <= 0) {
            return false;
        }
        event += written;
        left -= static_cast<size_t>(written);
    }
    return true;
}
