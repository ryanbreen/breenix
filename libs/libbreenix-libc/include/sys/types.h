/* sys/types.h: POSIX.1-2024 data types. */
#ifndef _SYS_TYPES_H
#define _SYS_TYPES_H

#include <__breenix/types.h>
#define __need_size_t
#include <stddef.h>

typedef __breenix_ssize_t ssize_t;
typedef __breenix_off_t off_t;
typedef __breenix_pid_t pid_t;
typedef __breenix_uid_t uid_t;
typedef __breenix_gid_t gid_t;
typedef __breenix_id_t id_t;
typedef __breenix_mode_t mode_t;
typedef __breenix_dev_t dev_t;
typedef __breenix_ino_t ino_t;
typedef __breenix_nlink_t nlink_t;
typedef __breenix_blksize_t blksize_t;
typedef __breenix_blkcnt_t blkcnt_t;
typedef __breenix_fsblkcnt_t fsblkcnt_t;
typedef __breenix_fsfilcnt_t fsfilcnt_t;
typedef __breenix_time_t time_t;
typedef __breenix_suseconds_t suseconds_t;
typedef __breenix_useconds_t useconds_t;
typedef __breenix_clock_t clock_t;
typedef __breenix_clockid_t clockid_t;
typedef __breenix_timer_t timer_t;
typedef __breenix_key_t key_t;

typedef __breenix_pthread_t pthread_t;
typedef __breenix_pthread_key_t pthread_key_t;
typedef __breenix_pthread_once_t pthread_once_t;
typedef __breenix_pthread_spinlock_t pthread_spinlock_t;
typedef __breenix_pthread_attr_t pthread_attr_t;
typedef __breenix_pthread_mutex_t pthread_mutex_t;
typedef __breenix_pthread_mutexattr_t pthread_mutexattr_t;
typedef __breenix_pthread_cond_t pthread_cond_t;
typedef __breenix_pthread_condattr_t pthread_condattr_t;
typedef __breenix_pthread_rwlock_t pthread_rwlock_t;
typedef __breenix_pthread_rwlockattr_t pthread_rwlockattr_t;
typedef __breenix_pthread_barrier_t pthread_barrier_t;
typedef __breenix_pthread_barrierattr_t pthread_barrierattr_t;

#ifdef __BREENIX_EXT
typedef unsigned char u_char;
typedef unsigned short u_short;
typedef unsigned int u_int;
typedef unsigned long u_long;
typedef unsigned short ushort;
typedef unsigned int uint;
typedef unsigned long ulong;
typedef long long quad_t;
typedef unsigned long long u_quad_t;
typedef char *caddr_t;
#include <stdint.h>
typedef __UINT8_TYPE__ u_int8_t;
typedef __UINT16_TYPE__ u_int16_t;
typedef __UINT32_TYPE__ u_int32_t;
typedef __UINT64_TYPE__ u_int64_t;
typedef off_t loff_t;
typedef off_t off64_t;
typedef ino_t ino64_t;
typedef blkcnt_t blkcnt64_t;
#include <endian.h>
#include <sys/select.h>
#include <sys/sysmacros.h>
#endif

#endif
