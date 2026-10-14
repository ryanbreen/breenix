/*
 * Compile-time checks that Breenix's C headers lay out the types the kernel and the Rust
 * runtime share exactly as they do (kernel/src, libs/libc/src/unix/breenix/mod.rs).
 * build.sh compiles this for both architectures on every build; it has no code. Each
 * lane adds the checks for the types its headers define.
 */
#define _POSIX_C_SOURCE 202405L
#include <dirent.h>
#include <fcntl.h>
#include <getopt.h>
#include <glob.h>
#include <locale.h>
#include <pthread.h>
#include <regex.h>
#include <sched.h>
#include <semaphore.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/sysmacros.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <wchar.h>
#include <wctype.h>
#include <__breenix/iovec.h>
#include <__breenix/timespec.h>
#include <__breenix/timeval.h>

#define SIZE(type, bytes) _Static_assert(sizeof(type) == (bytes), "sizeof " #type)
#define ALIGN(type, bytes) _Static_assert(_Alignof(type) == (bytes), "alignment of " #type)
#define AT(type, member, offset) \
    _Static_assert(offsetof(type, member) == (offset), "offset of " #type "." #member)

/* Scalars (LP64 on both architectures). */
SIZE(ssize_t, 8);
SIZE(off_t, 8);
SIZE(pid_t, 4);
SIZE(uid_t, 4);
SIZE(gid_t, 4);
SIZE(mode_t, 4);
SIZE(dev_t, 8);
SIZE(ino_t, 8);
SIZE(blkcnt_t, 8);
SIZE(time_t, 8);
SIZE(clock_t, 8);
SIZE(clockid_t, 4);
SIZE(__breenix_sigset_t, 8);
SIZE(__breenix_socklen_t, 4);
SIZE(__breenix_sa_family_t, 2);
#if defined(__x86_64__)
SIZE(nlink_t, 8);
SIZE(blksize_t, 8);
#else
SIZE(nlink_t, 4);
SIZE(blksize_t, 4);
#endif

/* Threads, semaphores and multibyte state, as the Rust runtime sizes them. */
SIZE(pthread_t, 8);
SIZE(pthread_key_t, 4);
SIZE(pthread_once_t, 4);
SIZE(pthread_attr_t, 56);       ALIGN(pthread_attr_t, 8);
SIZE(pthread_mutex_t, 40);      ALIGN(pthread_mutex_t, 8);
SIZE(pthread_mutexattr_t, 8);   ALIGN(pthread_mutexattr_t, 8);
SIZE(pthread_cond_t, 48);       ALIGN(pthread_cond_t, 8);
SIZE(pthread_condattr_t, 4);    ALIGN(pthread_condattr_t, 4);
SIZE(pthread_rwlock_t, 56);     ALIGN(pthread_rwlock_t, 8);
SIZE(pthread_rwlockattr_t, 8);  ALIGN(pthread_rwlockattr_t, 8);
SIZE(pthread_barrier_t, 32);    ALIGN(pthread_barrier_t, 4);
SIZE(pthread_barrierattr_t, 4); ALIGN(pthread_barrierattr_t, 4);
SIZE(__breenix_sem_t, 32);      ALIGN(__breenix_sem_t, 4);
SIZE(__breenix_mbstate_t, 8);   ALIGN(__breenix_mbstate_t, 4);
SIZE(mbstate_t, 8);            ALIGN(mbstate_t, 4);
SIZE(wchar_t, 4);
SIZE(wint_t, 4);
SIZE(wctype_t, 8);
SIZE(wctrans_t, 8);
SIZE(locale_t, 8);
SIZE(struct lconv, 96);       ALIGN(struct lconv, 8);
AT(struct lconv, int_frac_digits, 80);
AT(struct lconv, int_n_sign_posn, 93);
SIZE(struct tm, 56);          ALIGN(struct tm, 8);
AT(struct tm, tm_gmtoff, 40);
AT(struct tm, tm_zone, 48);

/* Times and buffers. */
SIZE(struct timespec, 16);
AT(struct timespec, tv_nsec, 8);
SIZE(struct timeval, 16);
AT(struct timeval, tv_usec, 8);
SIZE(struct iovec, 16);
AT(struct iovec, iov_len, 8);

/* struct flock (fcntl F_GETLK/F_SETLK). */
SIZE(struct flock, 32);
AT(struct flock, l_whence, 2);
AT(struct flock, l_start, 8);
AT(struct flock, l_len, 16);
AT(struct flock, l_pid, 24);

/* struct stat: Linux's x86_64 layout, and asm-generic on aarch64. */
#if defined(__x86_64__)
SIZE(struct stat, 144);
AT(struct stat, st_nlink, 16);
AT(struct stat, st_mode, 24);
AT(struct stat, st_uid, 28);
AT(struct stat, st_rdev, 40);
AT(struct stat, st_size, 48);
AT(struct stat, st_blksize, 56);
AT(struct stat, st_blocks, 64);
AT(struct stat, st_atim, 72);
AT(struct stat, st_mtim, 88);
AT(struct stat, st_ctim, 104);
#else
SIZE(struct stat, 128);
AT(struct stat, st_mode, 16);
AT(struct stat, st_nlink, 20);
AT(struct stat, st_uid, 24);
AT(struct stat, st_rdev, 32);
AT(struct stat, st_size, 48);
AT(struct stat, st_blksize, 56);
AT(struct stat, st_blocks, 64);
AT(struct stat, st_atim, 72);
AT(struct stat, st_mtim, 88);
AT(struct stat, st_ctim, 104);
#endif
AT(struct stat, st_ino, 8);

/* struct dirent: the library's Dirent (src/dirent.rs) and the Rust runtime's dirent. */
SIZE(struct dirent, 280);
AT(struct dirent, d_off, 8);
AT(struct dirent, d_reclen, 16);
AT(struct dirent, d_type, 18);
AT(struct dirent, d_name, 19);

/* glob_t, regex_t, regmatch_t and struct option, as src/glob.rs, src/regex and
   src/getopt.rs read them. */
SIZE(glob_t, 32);
AT(glob_t, gl_pathv, 8);
AT(glob_t, gl_offs, 16);
AT(glob_t, gl_flags, 24);
SIZE(regex_t, 24);
SIZE(regmatch_t, 16);
AT(regmatch_t, rm_eo, 8);
SIZE(struct option, 32);
AT(struct option, has_arg, 8);
AT(struct option, flag, 16);
AT(struct option, val, 24);

/* STDIO positions retain both the offset and the 8-byte multibyte state. */
SIZE(fpos_t, 16); ALIGN(fpos_t, 8); AT(fpos_t, offset, 0); AT(fpos_t, state, 8);

/* PTHREAD owns these public signal and cleanup layouts. */
SIZE(pthread_spinlock_t, 4); ALIGN(pthread_spinlock_t, 4);
SIZE(sem_t, 32); ALIGN(sem_t, 4);
SIZE(sigset_t, 8); ALIGN(sigset_t, 8);
SIZE(struct sched_param, 4);
SIZE(struct __breenix_cleanup, 24);
AT(struct __breenix_cleanup, routine, 8);
AT(struct __breenix_cleanup, arg, 16);
SIZE(struct sigaction, 32);
AT(struct sigaction, sa_mask, 8);
AT(struct sigaction, sa_flags, 16);
AT(struct sigaction, sa_restorer, 24);
SIZE(siginfo_t, 128); ALIGN(siginfo_t, 8);
/* Linux siginfo layout: the sender and fault fields and SIGPOLL's band start at 16. */
AT(siginfo_t, si_pid, 16); AT(siginfo_t, si_uid, 20); AT(siginfo_t, si_value, 24);
AT(siginfo_t, si_addr, 16); AT(siginfo_t, si_band, 16);
SIZE(struct sigevent, 64); ALIGN(struct sigevent, 8);
SIZE(stack_t, 24);
AT(stack_t, ss_flags, 8); AT(stack_t, ss_size, 16);
#if defined(__aarch64__)
SIZE(mcontext_t, 4384); ALIGN(mcontext_t, 16);
SIZE(ucontext_t, 4560); ALIGN(ucontext_t, 16);
AT(ucontext_t, uc_sigmask, 40);
AT(ucontext_t, uc_mcontext, 176);
AT(mcontext_t, pc, 264);
#else
SIZE(mcontext_t, 256); ALIGN(mcontext_t, 8);
SIZE(ucontext_t, 304); ALIGN(ucontext_t, 8);
AT(ucontext_t, uc_mcontext, 40);
AT(ucontext_t, uc_sigmask, 296);
#endif

/* POSIX statvfs has the eleven LP64 fields src/statfs.rs implements; no
 * unused foreign-layout reservation. Device masks match the kernel's 12/20 bits. */
SIZE(struct statvfs, 88);
AT(struct statvfs, f_flag, 72);
AT(struct statvfs, f_namemax, 80);
_Static_assert(major(makedev(0xabc, 0xdef01)) == 0xabc, "kernel device major");
_Static_assert(minor(makedev(0xabc, 0xdef01)) == 0xdef01, "kernel device minor");
