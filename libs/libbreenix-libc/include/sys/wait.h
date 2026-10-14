/* sys/wait.h: POSIX.1-2024 declarations for waiting, with Linux's status encoding. */
#ifndef _SYS_WAIT_H
#define _SYS_WAIT_H

#include <__breenix/types.h>

typedef __breenix_pid_t pid_t;
typedef __breenix_id_t id_t;

#define WNOHANG 1
#define WUNTRACED 2
#define WSTOPPED 2
#define WEXITED 4
#define WCONTINUED 8
#define WNOWAIT 0x1000000

/* Linux status encoding: exit code in bits 8-15 with zero low bits; a terminating
 * signal in bits 0-6 (bit 7 core dump); 0x7f in the low byte when stopped, with the
 * stop signal above it; 0xffff when continued. */
#define WEXITSTATUS(s) (((s) & 0xff00) >> 8)
#define WTERMSIG(s) ((s) & 0x7f)
#define WSTOPSIG(s) WEXITSTATUS(s)
#define WIFEXITED(s) (((s) & 0x7f) == 0)
#define WIFSIGNALED(s) (((s) & 0x7f) != 0 && ((s) & 0x7f) != 0x7f)
#define WIFSTOPPED(s) (((s) & 0xff) == 0x7f)
#define WIFCONTINUED(s) ((s) == 0xffff)
#define WCOREDUMP(s) ((s) & 0x80)

typedef enum { P_ALL = 0, P_PID = 1, P_PGID = 2 } idtype_t;

__BREENIX_BEGIN_DECLS

pid_t wait(int *);
pid_t waitpid(pid_t, int *, int);

#ifdef __BREENIX_EXT
struct rusage;
pid_t wait3(int *, int, struct rusage *);
pid_t wait4(pid_t, int *, int, struct rusage *);
#endif

__BREENIX_END_DECLS

#endif
