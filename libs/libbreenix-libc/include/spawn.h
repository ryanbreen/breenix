/* Clean-room POSIX.1-2024 spawn interfaces; layout is Breenix's own ABI. */
#ifndef __BREENIX_SPAWN_H
#define __BREENIX_SPAWN_H
#include <__breenix/features.h>
#if __BREENIX_POSIX
#include <sys/types.h>
#include <signal.h>
#include <sched.h>
#include <stddef.h>
typedef struct {
    short __flags;
    int __pgroup;
    sigset_t __sigdefault, __sigmask;
    struct sched_param __schedparam;
    int __schedpolicy;
} posix_spawnattr_t;
typedef struct { void *__head, *__tail; } posix_spawn_file_actions_t;
#if defined(__cplusplus)
#define __BREENIX_SPAWN_ASSERT static_assert
#else
#define __BREENIX_SPAWN_ASSERT _Static_assert
#endif
__BREENIX_SPAWN_ASSERT(sizeof(posix_spawnattr_t) == 32, "spawn attr ABI size");
__BREENIX_SPAWN_ASSERT(offsetof(posix_spawnattr_t, __sigdefault) == 8, "spawn signal ABI");
__BREENIX_SPAWN_ASSERT(offsetof(posix_spawnattr_t, __schedpolicy) == 28, "spawn scheduler ABI");
__BREENIX_SPAWN_ASSERT(sizeof(posix_spawn_file_actions_t) == 16, "spawn actions ABI size");
#undef __BREENIX_SPAWN_ASSERT
#define POSIX_SPAWN_RESETIDS 1
#define POSIX_SPAWN_SETPGROUP 2
#define POSIX_SPAWN_SETSIGDEF 4
#define POSIX_SPAWN_SETSIGMASK 8
#define POSIX_SPAWN_SETSCHEDPARAM 16
#define POSIX_SPAWN_SETSCHEDULER 32
#define POSIX_SPAWN_SETSID 64
__BREENIX_BEGIN_DECLS
int posix_spawn(pid_t *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT,
    const posix_spawn_file_actions_t *, const posix_spawnattr_t *__BREENIX_RESTRICT,
    char *const [], char *const []);
int posix_spawnp(pid_t *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT,
    const posix_spawn_file_actions_t *, const posix_spawnattr_t *__BREENIX_RESTRICT,
    char *const [], char *const []);
int posix_spawnattr_init(posix_spawnattr_t *);
int posix_spawnattr_destroy(posix_spawnattr_t *);
int posix_spawn_file_actions_init(posix_spawn_file_actions_t *);
int posix_spawn_file_actions_destroy(posix_spawn_file_actions_t *);
int posix_spawnattr_getflags(const posix_spawnattr_t *__BREENIX_RESTRICT, short *__BREENIX_RESTRICT);
int posix_spawnattr_setflags(posix_spawnattr_t *, short);
int posix_spawnattr_getpgroup(const posix_spawnattr_t *__BREENIX_RESTRICT, pid_t *__BREENIX_RESTRICT);
int posix_spawnattr_setpgroup(posix_spawnattr_t *, pid_t);
int posix_spawnattr_getsigdefault(const posix_spawnattr_t *__BREENIX_RESTRICT, sigset_t *__BREENIX_RESTRICT);
int posix_spawnattr_setsigdefault(posix_spawnattr_t *, const sigset_t *);
int posix_spawnattr_getsigmask(const posix_spawnattr_t *__BREENIX_RESTRICT, sigset_t *__BREENIX_RESTRICT);
int posix_spawnattr_setsigmask(posix_spawnattr_t *, const sigset_t *);
int posix_spawnattr_getschedparam(const posix_spawnattr_t *__BREENIX_RESTRICT, struct sched_param *__BREENIX_RESTRICT);
int posix_spawnattr_setschedparam(posix_spawnattr_t *, const struct sched_param *);
int posix_spawnattr_getschedpolicy(const posix_spawnattr_t *__BREENIX_RESTRICT, int *__BREENIX_RESTRICT);
int posix_spawnattr_setschedpolicy(posix_spawnattr_t *, int);
int posix_spawn_file_actions_addclose(posix_spawn_file_actions_t *, int);
int posix_spawn_file_actions_adddup2(posix_spawn_file_actions_t *, int, int);
int posix_spawn_file_actions_addopen(posix_spawn_file_actions_t *__BREENIX_RESTRICT, int, const char *__BREENIX_RESTRICT, int, mode_t);
int posix_spawn_file_actions_addchdir(posix_spawn_file_actions_t *__BREENIX_RESTRICT, const char *__BREENIX_RESTRICT);
int posix_spawn_file_actions_addfchdir(posix_spawn_file_actions_t *, int);
__BREENIX_END_DECLS
#endif
#endif
