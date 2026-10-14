#ifndef __BREENIX_SIGNAL_H
#define __BREENIX_SIGNAL_H
#include <__breenix/features.h>
typedef int sig_atomic_t;
#include <stddef.h>
#define SIG_DFL ((void (*)(int))0)
#define SIG_IGN ((void (*)(int))1)
#define SIG_ERR ((void (*)(int))-1)
#define SIGINT 2
#define SIGILL 4
#define SIGABRT 6
#define SIGFPE 8
#define SIGSEGV 11
#define SIGTERM 15
#if __BREENIX_POSIX
#include <__breenix/types.h>
#include <sys/types.h>
#include <__breenix/timespec.h>
typedef __breenix_sigset_t sigset_t;
#define SIGHUP 1
#define SIGQUIT 3
#define SIGTRAP 5
#define SIGBUS 7
#define SIGKILL 9
#define SIGUSR1 10
#define SIGUSR2 12
#define SIGPIPE 13
#define SIGALRM 14
#define SIGSTKFLT 16
#define SIGCHLD 17
#define SIGCONT 18
#define SIGSTOP 19
#define SIGTSTP 20
#define SIGTTIN 21
#define SIGTTOU 22
#define SIGURG 23
#define SIGXCPU 24
#define SIGXFSZ 25
#define SIGVTALRM 26
#define SIGPROF 27
#define SIGWINCH 28
#define SIGIO 29
#define SIGPWR 30
#define SIGSYS 31
/* Signal 32 is reserved for libc cancellation. */
#define SIGRTMIN 33
#define SIGRTMAX 64
#define NSIG 65
#define SIGIOT SIGABRT
#define SIGCLD SIGCHLD
#define SIGPOLL SIGIO
#define SIG_BLOCK 0
#define SIG_UNBLOCK 1
#define SIG_SETMASK 2
#define SA_NOCLDSTOP 1
#define SA_NOCLDWAIT 2
#define SA_SIGINFO 4
#define SA_RESTORER 0x04000000
#define SA_ONSTACK 0x08000000
#define SA_RESTART 0x10000000
#define SA_NODEFER 0x40000000
#define SA_RESETHAND 0x80000000U
#define SS_ONSTACK 1
#define SS_DISABLE 2
#ifdef __aarch64__
#define MINSIGSTKSZ 5120
#define SIGSTKSZ 16384
#else
#define MINSIGSTKSZ 2048
#define SIGSTKSZ 8192
#endif
#define SI_USER 0
#define SI_QUEUE (-1)
#define SI_TIMER (-2)
#define SI_MESGQ (-3)
#define SI_TKILL (-6)
#define SI_KERNEL 128
#define CLD_EXITED 1
#define CLD_KILLED 2
#define CLD_DUMPED 3
#define CLD_TRAPPED 4
#define CLD_STOPPED 5
#define CLD_CONTINUED 6
#define SIGEV_SIGNAL 0
#define SIGEV_NONE 1
#define SIGEV_THREAD 2
union sigval { int sival_int; void *sival_ptr; };
typedef struct {
    int si_signo, si_errno, si_code;
    int __padding;
    union {
        struct { pid_t pid; unsigned int uid; union sigval value; } sender;
        struct { pid_t pid; unsigned int uid; int status; } child;
        struct { void *addr; } fault;
        unsigned long __words[14];
    } __data;
} siginfo_t;
#define si_pid __data.sender.pid
#define si_uid __data.sender.uid
#define si_value __data.sender.value
#define si_status __data.child.status
#define si_addr __data.fault.addr
struct sigaction {
    union {
        void (*handler)(int);
        void (*action)(int, siginfo_t *, void *);
    } __handler;
    sigset_t sa_mask;
    int sa_flags;
    void (*sa_restorer)(void);
};
#define sa_handler __handler.handler
#define sa_sigaction __handler.action
typedef struct { void *ss_sp; int ss_flags; size_t ss_size; } stack_t;
#ifdef __aarch64__
typedef struct {
    unsigned long fault_address, regs[31], sp, pc, pstate;
    _Alignas(16) unsigned char __reserved[4096];
} mcontext_t;
typedef struct ucontext_t {
    unsigned long uc_flags;
    struct ucontext_t *uc_link;
    stack_t uc_stack;
    sigset_t uc_sigmask;
    unsigned char __unused[120];
    mcontext_t uc_mcontext;
} ucontext_t;
#else
typedef struct {
    unsigned long gregs[18];
    unsigned short cs, gs, fs, ss;
    unsigned long err, trapno, oldmask, cr2;
    void *fpregs;
    unsigned long __reserved[8];
} mcontext_t;
typedef struct ucontext_t {
    unsigned long uc_flags;
    struct ucontext_t *uc_link;
    stack_t uc_stack;
    mcontext_t uc_mcontext;
    sigset_t uc_sigmask;
} ucontext_t;
#endif
struct sigevent {
    union sigval sigev_value;
    int sigev_signo, sigev_notify;
    union {
        struct { void (*function)(union sigval); __breenix_pthread_attr_t *attributes; } thread;
        int thread_id;
        unsigned char __storage[48];
    } __notify;
};
#define sigev_notify_function __notify.thread.function
#define sigev_notify_attributes __notify.thread.attributes
#define sigev_notify_thread_id __notify.thread_id
#ifdef __cplusplus
extern "C" {
#endif
int kill(pid_t, int);
int sigaction(int, const struct sigaction *, struct sigaction *);
int sigprocmask(int, const sigset_t *, sigset_t *);
int sigemptyset(sigset_t *);
int sigfillset(sigset_t *);
int sigaddset(sigset_t *, int);
int sigdelset(sigset_t *, int);
int sigismember(const sigset_t *, int);
int sigpending(sigset_t *);
int sigsuspend(const sigset_t *);
int sigwait(const sigset_t *, int *);
int sigwaitinfo(const sigset_t *, siginfo_t *);
int sigtimedwait(const sigset_t *, siginfo_t *, const struct timespec *);
int sigaltstack(const stack_t *, stack_t *);
/* Avoid making <signal.h> depend on <pthread.h>. */
int pthread_kill(__breenix_pthread_t, int);
int pthread_sigmask(int, const sigset_t *, sigset_t *);
#ifdef __cplusplus
}
#endif
#endif /* __BREENIX_POSIX */
__BREENIX_BEGIN_DECLS
void (*signal(int, void (*)(int)))(int);
int raise(int);
__BREENIX_END_DECLS
#endif
