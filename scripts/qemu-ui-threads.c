// Loaded into QEMU by scripts/qemu-ui-launch.c (DYLD_INSERT_LIBRARIES): every thread QEMU creates
// without a QoS class of its own, its vCPU threads among them, asks for the user-interactive class
// as it starts.
//
// A thread's class cannot be set from outside its process without privilege, and QEMU creates its
// vCPU threads from its own qemu_main thread, so neither the launcher's role nor its own class
// reaches them: they ran at the default priority, 31. This wraps pthread_create so the new thread
// sets its own class before it runs QEMU's code. A thread created with an explicit class keeps it.
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <pthread/qos.h>
#include <stdlib.h>
#include <string.h>

struct start {
    void *(*routine)(void *);
    void *arg;
};

static void *start_user_interactive(void *p) {
    struct start start = *(struct start *)p;
    free(p);
    pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    return start.routine(start.arg);
}

static int create_user_interactive(pthread_t *thread, const pthread_attr_t *attr, void *(*routine)(void *),
                                   void *arg) {
    qos_class_t requested = QOS_CLASS_UNSPECIFIED;
    int relative = 0;
    if (attr != NULL && pthread_attr_get_qos_class_np((pthread_attr_t *)attr, &requested, &relative) == 0 &&
        requested != QOS_CLASS_UNSPECIFIED)
        return pthread_create(thread, attr, routine, arg);
    struct start *start = malloc(sizeof *start);
    if (start == NULL)
        return EAGAIN;
    start->routine = routine;
    start->arg = arg;
    int err = pthread_create(thread, attr, start_user_interactive, start);
    if (err != 0)
        free(start);
    return err;
}

// dyld replaces pthread_create with create_user_interactive everywhere but in this image.
__attribute__((used, section("__DATA,__interpose"))) static const struct {
    const void *replacement;
    const void *replacee;
} interpose[] = {{(const void *)create_user_interactive, (const void *)pthread_create}};

// Programs QEMU starts are not launched through this; take this library back out of the environment.
__attribute__((constructor)) static void forget_insertion(void) {
    Dl_info self;
    const char *inserted = getenv("DYLD_INSERT_LIBRARIES");
    if (inserted == NULL || dladdr((const void *)forget_insertion, &self) == 0 || self.dli_fname == NULL)
        return;
    size_t length = strlen(self.dli_fname);
    if (strncmp(inserted, self.dli_fname, length) != 0)
        return;
    if (inserted[length] == ':')
        setenv("DYLD_INSERT_LIBRARIES", inserted + length + 1, 1);
    else if (inserted[length] == '\0')
        unsetenv("DYLD_INSERT_LIBRARIES");
}
