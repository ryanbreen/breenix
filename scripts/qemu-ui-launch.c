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
// The program is exec'd in place, so it keeps this process ID.
#include <errno.h>
#include <libgen.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <mach/mach.h>
#include <mach/task_policy.h>
#include <pthread/qos.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <program> [arguments...]\n", argv[0]);
        return 2;
    }
    char self[PATH_MAX], real[PATH_MAX], threads[PATH_MAX];
    uint32_t size = sizeof self;
    if (_NSGetExecutablePath(self, &size) != 0 || realpath(self, real) == NULL ||
        snprintf(threads, sizeof threads, "%s/qemu-ui-threads.dylib", dirname(real)) >= (int)sizeof threads ||
        access(threads, R_OK) != 0) {
        fprintf(stderr, "%s: cannot find qemu-ui-threads.dylib beside this launcher\n", argv[0]);
        return 126;
    }
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
    struct task_category_policy role = {.role = TASK_DEFAULT_APPLICATION};
    kern_return_t kr = task_policy_set(mach_task_self(), TASK_CATEGORY_POLICY, (task_policy_t)&role,
                                       TASK_CATEGORY_POLICY_COUNT);
    if (kr != KERN_SUCCESS) {
        fprintf(stderr, "%s: cannot take the application role: %s\n", argv[0], mach_error_string(kr));
        return 126;
    }
    int err = pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    if (err != 0) {
        fprintf(stderr, "%s: cannot ask for the user-interactive class: %s\n", argv[0], strerror(err));
        return 126;
    }
    execvp(argv[1], argv + 1);
    fprintf(stderr, "%s: %s: %s\n", argv[0], argv[1], strerror(errno));
    return errno == ENOENT ? 127 : 126;
}
