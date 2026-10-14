/*
 * <dirent.h>: directory streams (POSIX.1-2024 XBD <dirent.h>), with Linux's struct dirent.
 */
#ifndef _DIRENT_H
#define _DIRENT_H

#include <__breenix/features.h>
#include <__breenix/types.h>

__BREENIX_BEGIN_DECLS

typedef __breenix_ino_t ino_t;
typedef __breenix_size_t size_t;
typedef __breenix_ssize_t ssize_t;

typedef struct __breenix_dir DIR;

struct dirent {
    ino_t d_ino;
    __breenix_off_t d_off;
    unsigned short d_reclen;
    unsigned char d_type;
    char d_name[256];
};

#define DT_UNKNOWN 0
#define DT_FIFO 1
#define DT_CHR 2
#define DT_DIR 4
#define DT_BLK 6
#define DT_REG 8
#define DT_LNK 10
#define DT_SOCK 12

int alphasort(const struct dirent **, const struct dirent **);
int closedir(DIR *);
int dirfd(DIR *);
DIR *fdopendir(int);
DIR *opendir(const char *);
struct dirent *readdir(DIR *);
int readdir_r(DIR *__BREENIX_RESTRICT, struct dirent *__BREENIX_RESTRICT, struct dirent **__BREENIX_RESTRICT);
void rewinddir(DIR *);
int scandir(const char *, struct dirent ***, int (*)(const struct dirent *),
            int (*)(const struct dirent **, const struct dirent **));
void seekdir(DIR *, long);
long telldir(DIR *);

__BREENIX_END_DECLS

#endif
