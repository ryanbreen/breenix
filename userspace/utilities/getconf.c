/*
 * getconf - get configuration values (POSIX.1-2024 XCU getconf)
 *
 *   getconf [-v specification] system_var
 *   getconf [-v specification] path_var pathname
 *
 * A system variable is answered by confstr() or sysconf(), a path variable by
 * pathconf() on the pathname. A variable that is valid but has no limit is
 * written as "undefined". An unknown name is a diagnostic and exit status 1.
 */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum kind { SYSCONF, CONFSTR, PATHCONF };

struct variable {
    const char *name;
    enum kind kind;
    int key;
};

static const struct variable variables[] = {
    { "ARG_MAX", SYSCONF, _SC_ARG_MAX },
    { "CHILD_MAX", SYSCONF, _SC_CHILD_MAX },
    { "CLK_TCK", SYSCONF, _SC_CLK_TCK },
    { "NGROUPS_MAX", SYSCONF, _SC_NGROUPS_MAX },
    { "OPEN_MAX", SYSCONF, _SC_OPEN_MAX },
    { "STREAM_MAX", SYSCONF, _SC_STREAM_MAX },
    { "TZNAME_MAX", SYSCONF, _SC_TZNAME_MAX },
    { "PAGESIZE", SYSCONF, _SC_PAGESIZE },
    { "PAGE_SIZE", SYSCONF, _SC_PAGE_SIZE },
    { "LINE_MAX", SYSCONF, _SC_LINE_MAX },
    { "LOGIN_NAME_MAX", SYSCONF, _SC_LOGIN_NAME_MAX },
    { "TTY_NAME_MAX", SYSCONF, _SC_TTY_NAME_MAX },
    { "HOST_NAME_MAX", SYSCONF, _SC_HOST_NAME_MAX },
    { "RTSIG_MAX", SYSCONF, _SC_RTSIG_MAX },
    { "SEM_NSEMS_MAX", SYSCONF, _SC_SEM_NSEMS_MAX },
    { "SEM_VALUE_MAX", SYSCONF, _SC_SEM_VALUE_MAX },
    { "SIGQUEUE_MAX", SYSCONF, _SC_SIGQUEUE_MAX },
    { "TIMER_MAX", SYSCONF, _SC_TIMER_MAX },
    { "GETGR_R_SIZE_MAX", SYSCONF, _SC_GETGR_R_SIZE_MAX },
    { "GETPW_R_SIZE_MAX", SYSCONF, _SC_GETPW_R_SIZE_MAX },
    { "PTHREAD_DESTRUCTOR_ITERATIONS", SYSCONF, _SC_THREAD_DESTRUCTOR_ITERATIONS },
    { "PTHREAD_KEYS_MAX", SYSCONF, _SC_THREAD_KEYS_MAX },
    { "PTHREAD_STACK_MIN", SYSCONF, _SC_THREAD_STACK_MIN },
    { "PTHREAD_THREADS_MAX", SYSCONF, _SC_THREAD_THREADS_MAX },
    { "_NPROCESSORS_CONF", SYSCONF, _SC_NPROCESSORS_CONF },
    { "_NPROCESSORS_ONLN", SYSCONF, _SC_NPROCESSORS_ONLN },
    { "_PHYS_PAGES", SYSCONF, _SC_PHYS_PAGES },
    { "_AVPHYS_PAGES", SYSCONF, _SC_AVPHYS_PAGES },
    { "_POSIX_JOB_CONTROL", SYSCONF, _SC_JOB_CONTROL },
    { "_POSIX_SAVED_IDS", SYSCONF, _SC_SAVED_IDS },
    { "_POSIX_VERSION", SYSCONF, _SC_VERSION },
    { "_POSIX_THREADS", SYSCONF, _SC_THREADS },
    { "_POSIX_MONOTONIC_CLOCK", SYSCONF, _SC_MONOTONIC_CLOCK },
    { "PATH", CONFSTR, _CS_PATH },
    { "LINK_MAX", PATHCONF, _PC_LINK_MAX },
    { "MAX_CANON", PATHCONF, _PC_MAX_CANON },
    { "MAX_INPUT", PATHCONF, _PC_MAX_INPUT },
    { "NAME_MAX", PATHCONF, _PC_NAME_MAX },
    { "PATH_MAX", PATHCONF, _PC_PATH_MAX },
    { "PIPE_BUF", PATHCONF, _PC_PIPE_BUF },
    { "_POSIX_CHOWN_RESTRICTED", PATHCONF, _PC_CHOWN_RESTRICTED },
    { "_POSIX_NO_TRUNC", PATHCONF, _PC_NO_TRUNC },
    { "_POSIX_VDISABLE", PATHCONF, _PC_VDISABLE },
};

/* The programming environments -v accepts: Breenix has only LP64. */
static const char *const specifications[] = {
    "POSIX_V8_LP64_OFF64",
    "POSIX_V7_LP64_OFF64",
};

static int usage(void)
{
    fputs("usage: getconf [-v specification] system_var\n"
          "       getconf [-v specification] path_var pathname\n", stderr);
    return 2;
}

static const struct variable *find(const char *name)
{
    for (size_t i = 0; i < sizeof(variables) / sizeof(variables[0]); i++) {
        if (strcmp(variables[i].name, name) == 0)
            return &variables[i];
    }
    return NULL;
}

/* Write a numeric limit: -1 with errno unchanged means no limit. */
static int print_limit(const char *name, long value)
{
    if (value == -1 && errno != 0) {
        fprintf(stderr, "getconf: %s: %s\n", name, strerror(errno));
        return 1;
    }
    if (value == -1)
        puts("undefined");
    else
        printf("%ld\n", value);
    return 0;
}

static int print_string(const char *name, int key)
{
    errno = 0;
    size_t len = confstr(key, NULL, 0);
    if (len == 0) {
        if (errno != 0) {
            fprintf(stderr, "getconf: %s: %s\n", name, strerror(errno));
            return 1;
        }
        puts("undefined");
        return 0;
    }
    char *value = malloc(len);
    if (value == NULL) {
        fprintf(stderr, "getconf: %s\n", strerror(errno));
        return 1;
    }
    confstr(key, value, len);
    puts(value);
    free(value);
    return 0;
}

int main(int argc, char **argv)
{
    int opt;
    while ((opt = getopt(argc, argv, "v:")) != -1) {
        if (opt != 'v')
            return usage();
        int known = 0;
        for (size_t i = 0; i < sizeof(specifications) / sizeof(specifications[0]); i++)
            known |= strcmp(specifications[i], optarg) == 0;
        if (!known) {
            fprintf(stderr, "getconf: unsupported specification: %s\n", optarg);
            return 1;
        }
    }
    argc -= optind;
    argv += optind;
    if (argc < 1 || argc > 2)
        return usage();

    const struct variable *var = find(argv[0]);
    if (var == NULL || (var->kind == PATHCONF) != (argc == 2)) {
        fprintf(stderr, "getconf: invalid variable: %s\n", argv[0]);
        return 1;
    }

    int status;
    errno = 0;
    switch (var->kind) {
    case SYSCONF:
        status = print_limit(var->name, sysconf(var->key));
        break;
    case CONFSTR:
        status = print_string(var->name, var->key);
        break;
    default:
        status = print_limit(var->name, pathconf(argv[1], var->key));
        break;
    }
    if (fflush(stdout) != 0 || ferror(stdout)) {
        fprintf(stderr, "getconf: write error\n");
        return 1;
    }
    return status;
}
