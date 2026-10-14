/* sys/stat.h: POSIX.1-2024 file status, in the kernel's layout for each architecture. */
#ifndef _SYS_STAT_H
#define _SYS_STAT_H

#include <__breenix/types.h>
#include <__breenix/timespec.h>

typedef __breenix_blkcnt_t blkcnt_t;
typedef __breenix_blksize_t blksize_t;
typedef __breenix_dev_t dev_t;
typedef __breenix_ino_t ino_t;
typedef __breenix_mode_t mode_t;
typedef __breenix_nlink_t nlink_t;
typedef __breenix_uid_t uid_t;
typedef __breenix_gid_t gid_t;
typedef __breenix_off_t off_t;
typedef __breenix_time_t time_t;

#if defined(__x86_64__)
/* Linux's x86_64 layout (144 bytes). */
struct stat {
    dev_t st_dev;
    ino_t st_ino;
    nlink_t st_nlink;
    mode_t st_mode;
    uid_t st_uid;
    gid_t st_gid;
    int __pad0;
    dev_t st_rdev;
    off_t st_size;
    blksize_t st_blksize;
    blkcnt_t st_blocks;
    struct timespec st_atim;
    struct timespec st_mtim;
    struct timespec st_ctim;
    long __unused[3];
};
#else
/* Linux's asm-generic layout (128 bytes). */
struct stat {
    dev_t st_dev;
    ino_t st_ino;
    mode_t st_mode;
    nlink_t st_nlink;
    uid_t st_uid;
    gid_t st_gid;
    dev_t st_rdev;
    dev_t __pad1;
    off_t st_size;
    blksize_t st_blksize;
    int __pad2;
    blkcnt_t st_blocks;
    struct timespec st_atim;
    struct timespec st_mtim;
    struct timespec st_ctim;
    int __unused[2];
};
#endif

#define st_atime st_atim.tv_sec
#define st_mtime st_mtim.tv_sec
#define st_ctime st_ctim.tv_sec

#define S_IFMT 0170000
#define S_IFSOCK 0140000
#define S_IFLNK 0120000
#define S_IFREG 0100000
#define S_IFBLK 0060000
#define S_IFDIR 0040000
#define S_IFCHR 0020000
#define S_IFIFO 0010000
#define S_ISUID 04000
#define S_ISGID 02000
#define S_ISVTX 01000
#define S_IRWXU 0700
#define S_IRUSR 0400
#define S_IWUSR 0200
#define S_IXUSR 0100
#define S_IRWXG 070
#define S_IRGRP 040
#define S_IWGRP 020
#define S_IXGRP 010
#define S_IRWXO 07
#define S_IROTH 04
#define S_IWOTH 02
#define S_IXOTH 01

#define S_ISBLK(m) (((m) & S_IFMT) == S_IFBLK)
#define S_ISCHR(m) (((m) & S_IFMT) == S_IFCHR)
#define S_ISDIR(m) (((m) & S_IFMT) == S_IFDIR)
#define S_ISFIFO(m) (((m) & S_IFMT) == S_IFIFO)
#define S_ISREG(m) (((m) & S_IFMT) == S_IFREG)
#define S_ISLNK(m) (((m) & S_IFMT) == S_IFLNK)
#define S_ISSOCK(m) (((m) & S_IFMT) == S_IFSOCK)
#define S_TYPEISMQ(buf) 0
#define S_TYPEISSEM(buf) 0
#define S_TYPEISSHM(buf) 0

#define UTIME_NOW ((1l << 30) - 1l)
#define UTIME_OMIT ((1l << 30) - 2l)

__BREENIX_BEGIN_DECLS

int chmod(const char *, mode_t);
int fchmod(int, mode_t);
int fchmodat(int, const char *, mode_t, int);
int fstat(int, struct stat *);
int fstatat(int, const char *__BREENIX_RESTRICT, struct stat *__BREENIX_RESTRICT, int);
int futimens(int, const struct timespec[2]);
int lstat(const char *__BREENIX_RESTRICT, struct stat *__BREENIX_RESTRICT);
int mkdir(const char *, mode_t);
int mkdirat(int, const char *, mode_t);
int mkfifo(const char *, mode_t);
int mkfifoat(int, const char *, mode_t);
int mknod(const char *, mode_t, dev_t);
int mknodat(int, const char *, mode_t, dev_t);
int stat(const char *__BREENIX_RESTRICT, struct stat *__BREENIX_RESTRICT);
mode_t umask(mode_t);
int utimensat(int, const char *, const struct timespec[2], int);

#ifdef __BREENIX_EXT
#define stat64 stat
#define fstat64 fstat
#define lstat64 lstat
#define fstatat64 fstatat
#define S_IREAD S_IRUSR
#define S_IWRITE S_IWUSR
#define S_IEXEC S_IXUSR
#endif

__BREENIX_END_DECLS

#endif
