/*
 * The ABI types Breenix's headers share, under reserved names. Public headers typedef
 * the POSIX names from these. Sizes follow the kernel's definitions on each
 * architecture (LP64 on both) and the Rust side in libs/libc/src/unix/breenix/mod.rs;
 * userspace/libc-suite/abi_check.c asserts them.
 *
 * The headers need C11 or later: a public typedef such as off_t may be repeated in
 * every header POSIX names it in, which C11 6.7p3 allows when the type is the same.
 */
#ifndef __BREENIX_TYPES_H
#define __BREENIX_TYPES_H

#include <__breenix/features.h>

typedef __SIZE_TYPE__ __breenix_size_t;
typedef long __breenix_ssize_t;
typedef long __breenix_off_t;
typedef int __breenix_pid_t;
typedef unsigned int __breenix_uid_t;
typedef unsigned int __breenix_gid_t;
typedef unsigned int __breenix_id_t;
typedef unsigned int __breenix_mode_t;
typedef unsigned long __breenix_dev_t;
typedef unsigned long __breenix_ino_t;
#if defined(__x86_64__)
typedef unsigned long __breenix_nlink_t;
typedef long __breenix_blksize_t;
#elif defined(__aarch64__)
typedef unsigned int __breenix_nlink_t;
typedef int __breenix_blksize_t;
#else
#error "Breenix supports x86_64 and aarch64"
#endif
typedef long __breenix_blkcnt_t;
typedef unsigned long __breenix_fsblkcnt_t;
typedef unsigned long __breenix_fsfilcnt_t;
typedef long __breenix_time_t;
typedef long __breenix_suseconds_t;
typedef unsigned int __breenix_useconds_t;
typedef long __breenix_clock_t;
typedef int __breenix_clockid_t;
typedef void *__breenix_timer_t;
typedef int __breenix_key_t;
typedef unsigned int __breenix_socklen_t;
typedef unsigned short __breenix_sa_family_t;
typedef unsigned long __breenix_rlim_t;
typedef unsigned long __breenix_nfds_t;
typedef unsigned int __breenix_in_addr_t;
typedef unsigned short __breenix_in_port_t;

/* One bit per signal 1..64: the kernel's 8-byte mask. */
typedef unsigned long __breenix_sigset_t;

/* Threads: sizes and alignments are the ones the Rust runtime uses; an all-zero object
 * is the statically initialised one. */
typedef unsigned long __breenix_pthread_t;
typedef unsigned int __breenix_pthread_key_t;
typedef int __breenix_pthread_once_t;
typedef int __breenix_pthread_spinlock_t;
typedef struct { char __opaque[56]; } __attribute__((aligned(8))) __breenix_pthread_attr_t;
typedef struct { char __opaque[40]; } __attribute__((aligned(8))) __breenix_pthread_mutex_t;
typedef struct { char __opaque[4]; } __attribute__((aligned(8))) __breenix_pthread_mutexattr_t;
typedef struct { char __opaque[48]; } __attribute__((aligned(8))) __breenix_pthread_cond_t;
typedef struct { char __opaque[4]; } __attribute__((aligned(4))) __breenix_pthread_condattr_t;
typedef struct { char __opaque[56]; } __attribute__((aligned(8))) __breenix_pthread_rwlock_t;
typedef struct { char __opaque[8]; } __attribute__((aligned(8))) __breenix_pthread_rwlockattr_t;
typedef struct { char __opaque[32]; } __attribute__((aligned(4))) __breenix_pthread_barrier_t;
typedef struct { char __opaque[4]; } __attribute__((aligned(4))) __breenix_pthread_barrierattr_t;
typedef struct { char __opaque[32]; } __attribute__((aligned(4))) __breenix_sem_t;

/* Locales and wide characters (LOCALE's src/locale/mod.rs owns the layouts). */
typedef void *__breenix_locale_t;
typedef struct {
    unsigned int __value;
    unsigned char __expected, __seen, __lead, __reserved;
} __breenix_mbstate_t;
typedef __WINT_TYPE__ __breenix_wint_t;
typedef unsigned long __breenix_wctype_t;
typedef const int *__breenix_wctrans_t;

#endif
