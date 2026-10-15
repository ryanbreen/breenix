/* <utmpx.h>: POSIX.1-2024 user accounting database. libc does not define this header's functions yet
 * (issue #1601), so it declares only the types and constants. */
#ifndef __BREENIX_UTMPX_H
#define __BREENIX_UTMPX_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <__breenix/types.h>
#include <__breenix/timeval.h>
typedef __breenix_pid_t pid_t;

struct utmpx {
    char ut_user[32];
    char ut_id[4];
    char ut_line[32];
    pid_t ut_pid;
    short ut_type;
    struct timeval ut_tv;
};

#define EMPTY 0
#define BOOT_TIME 2
#define NEW_TIME 3
#define OLD_TIME 4
#define INIT_PROCESS 5
#define LOGIN_PROCESS 6
#define USER_PROCESS 7
#define DEAD_PROCESS 8
#endif /* __BREENIX_POSIX */
#endif
