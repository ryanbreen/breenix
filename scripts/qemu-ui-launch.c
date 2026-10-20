// Runs QEMU with its threads in macOS's user-interactive priority band.
//
// Usage: qemu-ui-launch <qemu-system-*> [arguments...]
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
#include <fcntl.h>
#include <libproc.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <mach-o/fat.h>
#include <mach-o/loader.h>
#include <mach/mach.h>
#include <mach/task_policy.h>
#include <pthread.h>
#include <pthread/qos.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>

static int qemu_name(const char *path) {
    const char *name = strrchr(path, '/');
    name = name == NULL ? path : name + 1;
    return strncmp(name, "qemu-system-", 12) == 0 && name[12] != '\0';
}

// Resolve PATH before setting the injection environment. execvp's ENOEXEC shell
// fallback and interpreter scripts must not receive QEMU's library either.
static int resolve_qemu(const char *program, char *resolved) {
    if (!qemu_name(program)) goto refused;
    if (strchr(program, '/') != NULL) {
        if (realpath(program, resolved) == NULL) return errno == ENOENT ? 127 : 126;
    } else {
        const char *path = getenv("PATH");
        char *search = strdup(path == NULL ? "/usr/bin:/bin" : path);
        if (search == NULL) return 126;
        char *cursor = search, *part;
        int found = 0;
        while ((part = strsep(&cursor, ":")) != NULL) {
            char candidate[PATH_MAX];
            if (snprintf(candidate, sizeof candidate, "%s/%s", *part ? part : ".", program) < (int)sizeof candidate &&
                access(candidate, X_OK) == 0 && realpath(candidate, resolved) != NULL) {
                found = 1;
                break;
            }
        }
        free(search);
        if (!found) return 127;
    }
    if (!qemu_name(resolved) || access(resolved, X_OK) != 0) goto refused;
    int fd = open(resolved, O_RDONLY);
    if (fd < 0) return 126;
    uint32_t magic = 0;
    ssize_t count = read(fd, &magic, sizeof magic);
    close(fd);
    if (count == sizeof magic &&
        (magic == MH_MAGIC_64 || magic == MH_CIGAM_64 || magic == MH_MAGIC || magic == MH_CIGAM ||
         magic == FAT_MAGIC || magic == FAT_CIGAM || magic == FAT_MAGIC_64 || magic == FAT_CIGAM_64))
        return 0;
refused:
    fprintf(stderr, "qemu-ui-launch: refusing %s: expected a qemu-system-* executable; run wrappers outside the launcher\n", program);
    return 126;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <qemu-system-*> [arguments...]\n", argv[0]);
        return 2;
    }
    char qemu[PATH_MAX];
    int status = resolve_qemu(argv[1], qemu);
    if (status != 0) {
        fprintf(stderr, "%s: cannot launch %s (exit %d)\n", argv[0], argv[1], status);
        return status;
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
    execv(qemu, argv + 1);
    fprintf(stderr, "%s: %s: %s\n", argv[0], argv[1], strerror(errno));
    return errno == ENOENT ? 127 : 126;
}
