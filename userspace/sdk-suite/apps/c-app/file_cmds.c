#define _POSIX_C_SOURCE 200809L

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include "app.h"

int cmd_files(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    umask(022);
    int fd = open("f", O_CREAT | O_WRONLY | O_TRUNC, 0640);
    if (fd < 0) {
        return failed("open f for writing");
    }
    if (write(fd, "0123456789", 10) != 10) {
        return failed("write");
    }
    close(fd);
    fd = open("f", O_RDWR);
    if (fd < 0) {
        return failed("open f for reading");
    }
    if (lseek(fd, 4, SEEK_SET) != 4) {
        return failed("lseek");
    }
    char buf[4] = {0};
    if (read(fd, buf, 3) != 3) {
        return failed("read");
    }
    struct stat st;
    if (fstat(fd, &st) != 0) {
        return failed("fstat");
    }
    close(fd);
    printf("read=%s size=%lld mode=%o\n", buf, (long long)st.st_size, (unsigned)(st.st_mode & 0777));
    if (rename("f", "g") != 0) {
        return failed("rename");
    }
    if (access("f", F_OK) == 0 || access("g", F_OK) != 0) {
        printf("rename left f or lost g\n");
        return 1;
    }
    printf("renamed\n");
    if (unlink("g") != 0) {
        return failed("unlink");
    }
    if (access("g", F_OK) == 0 || errno != ENOENT) {
        printf("g still exists after unlink\n");
        return 1;
    }
    printf("unlinked\n");
    return 0;
}

static int compare(const void *a, const void *b)
{
    return strcmp(*(char *const *)a, *(char *const *)b);
}

int cmd_dirs(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    if (mkdir("d", 0755) != 0) {
        return failed("mkdir d");
    }
    const char *names[] = {"d/b", "d/c", "d/a"};
    for (int i = 0; i < 3; i++) {
        int fd = open(names[i], O_CREAT | O_WRONLY, 0644);
        if (fd < 0) {
            return failed("create a file in d");
        }
        close(fd);
    }
    DIR *dir = opendir("d");
    if (!dir) {
        return failed("opendir d");
    }
    char *seen[8];
    int count = 0;
    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL) {
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) {
            continue;
        }
        if (count < 8) {
            seen[count++] = strdup(entry->d_name);
        }
    }
    closedir(dir);
    qsort(seen, (size_t)count, sizeof seen[0], compare);
    printf("entries=");
    for (int i = 0; i < count; i++) {
        printf("%s%s", i ? "," : "", seen[i]);
        free(seen[i]);
    }
    printf("\n");
    for (int i = 0; i < 3; i++) {
        if (unlink(names[i]) != 0) {
            return failed("unlink a file in d");
        }
    }
    if (rmdir("d") != 0) {
        return failed("rmdir d");
    }
    printf("rmdir ok\n");
    return 0;
}
