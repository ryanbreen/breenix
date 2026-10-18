// Runs a program, normally QEMU, with its threads in macOS's user-interactive priority band.
//
// Usage: qemu-ui-launch <program> [arguments...]
// (built by scripts/qemu-ui-launch.sh, beside qemu-ui-threads.dylib)
//
// posix_spawnattr_set_qos_class_np cannot start a program at the user-interactive class: it
// accepts only the utility and background classes. Three things are needed instead, none of them
// privileged:
//   1. An application role. macOS squashes a user-interactive request to the default priority, 31,
//      in a process with no role, and a process started from a shell has none. This launcher gives
//      its own task the default application role (the role AppKit gives an app it cannot yet call
//      focal or non-focal); the role survives exec. A windowed QEMU later takes the role AppKit
//      assigns it, which also honours the class.
//   2. The class on the main thread, which this launcher asks for before exec.
//   3. The class on QEMU's other threads. They do not inherit the main thread's class, and QEMU
//      creates its vCPU threads from its own qemu_main thread, so qemu-ui-threads.dylib, inserted
//      with DYLD_INSERT_LIBRARIES, has each thread QEMU creates ask for the class as it starts.
// Ordinary launches exec in place. An inherited clamp needs a fresh launchd task
// supervised by qemu-unclamp.py; the caller's policy is never changed.
#include <errno.h>
#include <libproc.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <mach/mach.h>
#include <mach/task_policy.h>
#include <pthread.h>
#include <pthread/qos.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <program> [arguments...]\n", argv[0]);
        return 2;
    }
    char self[PATH_MAX], real[PATH_MAX], directory[PATH_MAX], threads[PATH_MAX];
    uint32_t size = sizeof self;
    if (_NSGetExecutablePath(self, &size) != 0 || realpath(self, real) == NULL) return 126;
    strcpy(directory, real);
    *strrchr(directory, '/') = '\0';
    if (snprintf(threads, sizeof threads, "%s/qemu-ui-threads.dylib", directory) >= (int)sizeof threads ||
        access(threads, R_OK) != 0) {
        fprintf(stderr, "%s: cannot find qemu-ui-threads.dylib beside this launcher\n", argv[0]);
        return 126;
    }
    // Builds may be backgrounded, including the lane that invokes this launcher.
    // Reset only this boot process; never change its parent's scheduling policy.
    // Raising an inherited nice value may require privilege. The fresh-task
    // path below restores it without changing or elevating the caller.
    int needs_fresh_task = setpriority(PRIO_DARWIN_PROCESS, 0, 0) != 0;
    if (setpriority(PRIO_PROCESS, 0, 0) != 0) needs_fresh_task = 1;
    struct task_qos_policy tiers = {LATENCY_QOS_TIER_UNSPECIFIED, THROUGHPUT_QOS_TIER_UNSPECIFIED};
    kern_return_t kr = task_policy_set(mach_task_self(), TASK_BASE_QOS_POLICY,
                                       (task_policy_t)&tiers, TASK_QOS_POLICY_COUNT);
    if (kr != KERN_SUCCESS) {
        fprintf(stderr, "%s: cannot reset inherited QoS tiers: %s\n", argv[0], mach_error_string(kr));
        return 126;
    }
    struct task_category_policy role = {.role = TASK_DEFAULT_APPLICATION};
    kr = task_policy_set(mach_task_self(), TASK_CATEGORY_POLICY, (task_policy_t)&role,
                         TASK_CATEGORY_POLICY_COUNT);
    int err = pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    if (kr != KERN_SUCCESS || err != 0) {
        fprintf(stderr, "%s: cannot restore boot QoS\n", argv[0]);
        return 126;
    }
    uint64_t tid;
    struct proc_threadinfo info;
    if (pthread_threadid_np(NULL, &tid) != 0 ||
        proc_pidinfo(getpid(), PROC_PIDTHREADID64INFO, tid, &info, sizeof info) != sizeof info) {
        fprintf(stderr, "%s: cannot verify boot priority\n", argv[0]);
        return 126;
    }
    if (needs_fresh_task || info.pth_priority < 46) {
        // A spawn QoS clamp survives fork and exec and cannot be revoked by
        // the task itself. launchd supplies a fresh task; the helper supervises
        // it and passes the original console descriptors without a relay.
        char helper[PATH_MAX];
        if (getenv("BREENIX_QEMU_UNCLAMPED") != NULL ||
            snprintf(helper, sizeof helper, "%s/qemu-unclamp.py", directory) >= (int)sizeof helper) {
            fprintf(stderr, "%s: boot remains clamped at priority %d\n", argv[0], info.pth_priority);
            return 126;
        }
        char **args = calloc((size_t)argc + 3, sizeof *args);
        if (args == NULL) return 126;
        args[0] = "python3";
        args[1] = helper;
        args[2] = real;
        for (int i = 1; i < argc; i++) args[i + 2] = argv[i];
        execvp(args[0], args);
        fprintf(stderr, "%s: cannot start unclamped boot: %s\n", argv[0], strerror(errno));
        return 126;
    }
    unsetenv("BREENIX_QEMU_UNCLAMPED");
    const char *inserted = getenv("DYLD_INSERT_LIBRARIES");
    char *libraries = NULL;
    if (inserted != NULL && *inserted != '\0' ? asprintf(&libraries, "%s:%s", threads, inserted) < 0
                                               : (libraries = strdup(threads)) == NULL) {
        fprintf(stderr, "%s: out of memory\n", argv[0]);
        return 126;
    }
    if (setenv("DYLD_INSERT_LIBRARIES", libraries, 1) != 0) {
        fprintf(stderr, "%s: cannot set DYLD_INSERT_LIBRARIES: %s\n", argv[0], strerror(errno));
        return 126;
    }
    execvp(argv[1], argv + 1);
    fprintf(stderr, "%s: %s: %s\n", argv[0], argv[1], strerror(errno));
    return errno == ENOENT ? 127 : 126;
}
