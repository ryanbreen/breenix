/* Run on a CPU without RNG instructions (for example cortex-a72), with one
 * vCPU. Compile with scripts/breenix-cc and run through probe --run. A busy
 * syscall would prevent the parent from running and fail the boot deadline.
 * Written against Breenix's syscall ABI and C interfaces. */
#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#if defined(__aarch64__)
#define NR_GETRANDOM 278
#else
#define NR_GETRANDOM 318
#endif

static volatile sig_atomic_t caught;
static void handler(int sig) { caught = sig; }
static long random_byte(unsigned flags) {
    unsigned char byte;
    return syscall(NR_GETRANDOM, &byte, 1UL, (unsigned long)flags);
}
static void pause_for_child(void) {
    struct timespec delay = {0, 100000000};
    while (nanosleep(&delay, &delay) < 0) assert(errno == EINTR);
}
static void check_wait(int sig) {
    int pipefd[2];
    assert(pipe(pipefd) == 0);
    pid_t pid = fork();
    assert(pid >= 0);
    if (pid == 0) {
        close(pipefd[0]);
        struct sigaction action = {0};
        action.sa_handler = handler;
        assert(sigemptyset(&action.sa_mask) == 0);
        assert(sigaction(SIGUSR1, &action, NULL) == 0);
        assert(write(pipefd[1], "R", 1) == 1);
        close(pipefd[1]);
        errno = 0;
        long result = random_byte(0);
        assert(sig == SIGUSR1 && result == -1 && errno == EINTR && caught == SIGUSR1);
        _exit(0);
    }
    close(pipefd[1]);
    char ready;
    assert(read(pipefd[0], &ready, 1) == 1 && ready == 'R');
    close(pipefd[0]);
    pause_for_child();
    int status = 0;
    assert(waitpid(pid, &status, WNOHANG) == 0);
    assert(kill(pid, sig) == 0);
    assert(waitpid(pid, &status, 0) == pid);
    if (sig == SIGKILL) assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
    else assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}
int main(void) {
    alarm(10);
    errno = 0;
    assert(random_byte(1) == -1 && errno == EAGAIN);
    puts("GETRANDOM_UNREADY: nonblock EAGAIN");
    assert(random_byte(4) == 1);
    check_wait(SIGUSR1);
    puts("GETRANDOM_UNREADY: caught signal EINTR");
    check_wait(SIGKILL);
    puts("GETRANDOM_UNREADY: blocked child killed and reaped");
    errno = 0;
    assert(random_byte(1) == -1 && errno == EAGAIN);
    puts("GETRANDOM_UNREADY: PASS (timing and device input earned no credit)");
    return 0;
}
