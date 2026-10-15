/*
 * getconf - get configuration values (POSIX.1-2024 XCU getconf)
 *
 *   getconf [-v specification] system_var
 *   getconf [-v specification] path_var pathname
 *
 * A system variable is answered by confstr() or sysconf(), or is one of the
 * <limits.h> constants; a path variable is answered by pathconf() on the
 * pathname. A variable that is valid but has no value is written as
 * "undefined". An unknown name is a diagnostic and exit status 1.
 *
 * -v names a programming environment. Breenix provides only the LP64 one with
 * a 64-bit off_t, whose values are the defaults, so -v accepts that
 * environment (POSIX_V8_LP64_OFF64, or its POSIX_V7 name) and diagnoses any
 * other.
 */
#define _XOPEN_SOURCE 800
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

enum kind { SYSCONF, CONFSTR, PATHCONF, CONSTANT, UCONSTANT };

struct variable {
    const char *name;
    enum kind kind;
    long long key; /* the query name, or the constant's value */
};

#define SC(name, key) { name, SYSCONF, key }
#define CS(name, key) { name, CONFSTR, key }
#define PC(name, key) { name, PATHCONF, key }
#define LIMIT(name) { #name, CONSTANT, name }
#define ULIMIT(name) { #name, UCONSTANT, (long long)name }

static const struct variable variables[] = {
    /* sysconf limits */
    SC("AIO_LISTIO_MAX", _SC_AIO_LISTIO_MAX),
    SC("AIO_MAX", _SC_AIO_MAX),
    SC("AIO_PRIO_DELTA_MAX", _SC_AIO_PRIO_DELTA_MAX),
    SC("ARG_MAX", _SC_ARG_MAX),
    SC("ATEXIT_MAX", _SC_ATEXIT_MAX),
    SC("BC_BASE_MAX", _SC_BC_BASE_MAX),
    SC("BC_DIM_MAX", _SC_BC_DIM_MAX),
    SC("BC_SCALE_MAX", _SC_BC_SCALE_MAX),
    SC("BC_STRING_MAX", _SC_BC_STRING_MAX),
    SC("CHILD_MAX", _SC_CHILD_MAX),
    SC("CLK_TCK", _SC_CLK_TCK),
    SC("COLL_WEIGHTS_MAX", _SC_COLL_WEIGHTS_MAX),
    SC("DELAYTIMER_MAX", _SC_DELAYTIMER_MAX),
    SC("EXPR_NEST_MAX", _SC_EXPR_NEST_MAX),
    SC("HOST_NAME_MAX", _SC_HOST_NAME_MAX),
    SC("IOV_MAX", _SC_IOV_MAX),
    SC("LINE_MAX", _SC_LINE_MAX),
    SC("LOGIN_NAME_MAX", _SC_LOGIN_NAME_MAX),
    SC("NGROUPS_MAX", _SC_NGROUPS_MAX),
    SC("MQ_OPEN_MAX", _SC_MQ_OPEN_MAX),
    SC("MQ_PRIO_MAX", _SC_MQ_PRIO_MAX),
    SC("OPEN_MAX", _SC_OPEN_MAX),
    SC("PAGESIZE", _SC_PAGESIZE),
    SC("PAGE_SIZE", _SC_PAGE_SIZE),
    SC("PTHREAD_DESTRUCTOR_ITERATIONS", _SC_THREAD_DESTRUCTOR_ITERATIONS),
    SC("PTHREAD_KEYS_MAX", _SC_THREAD_KEYS_MAX),
    SC("PTHREAD_STACK_MIN", _SC_THREAD_STACK_MIN),
    SC("PTHREAD_THREADS_MAX", _SC_THREAD_THREADS_MAX),
    SC("RE_DUP_MAX", _SC_RE_DUP_MAX),
    SC("RTSIG_MAX", _SC_RTSIG_MAX),
    SC("SEM_NSEMS_MAX", _SC_SEM_NSEMS_MAX),
    SC("SEM_VALUE_MAX", _SC_SEM_VALUE_MAX),
    SC("SIGQUEUE_MAX", _SC_SIGQUEUE_MAX),
    SC("STREAM_MAX", _SC_STREAM_MAX),
    SC("SYMLOOP_MAX", _SC_SYMLOOP_MAX),
    SC("TIMER_MAX", _SC_TIMER_MAX),
    SC("TTY_NAME_MAX", _SC_TTY_NAME_MAX),
    SC("TZNAME_MAX", _SC_TZNAME_MAX),
    SC("GETGR_R_SIZE_MAX", _SC_GETGR_R_SIZE_MAX),
    SC("GETPW_R_SIZE_MAX", _SC_GETPW_R_SIZE_MAX),
    SC("NPROCESSORS_CONF", _SC_NPROCESSORS_CONF),
    SC("_NPROCESSORS_CONF", _SC_NPROCESSORS_CONF),
    SC("NPROCESSORS_ONLN", _SC_NPROCESSORS_ONLN),
    SC("_NPROCESSORS_ONLN", _SC_NPROCESSORS_ONLN),
    SC("_PHYS_PAGES", _SC_PHYS_PAGES),
    SC("_AVPHYS_PAGES", _SC_AVPHYS_PAGES),
    /* sysconf options */
    SC("_POSIX_ADVISORY_INFO", _SC_ADVISORY_INFO),
    SC("_POSIX_ASYNCHRONOUS_IO", _SC_ASYNCHRONOUS_IO),
    SC("_POSIX_BARRIERS", _SC_BARRIERS),
    SC("_POSIX_CLOCK_SELECTION", _SC_CLOCK_SELECTION),
    SC("_POSIX_CPUTIME", _SC_CPUTIME),
    SC("_POSIX_FSYNC", _SC_FSYNC),
    SC("_POSIX_IPV6", _SC_IPV6),
    SC("_POSIX_JOB_CONTROL", _SC_JOB_CONTROL),
    SC("_POSIX_MAPPED_FILES", _SC_MAPPED_FILES),
    SC("_POSIX_MEMLOCK", _SC_MEMLOCK),
    SC("_POSIX_MEMLOCK_RANGE", _SC_MEMLOCK_RANGE),
    SC("_POSIX_MEMORY_PROTECTION", _SC_MEMORY_PROTECTION),
    SC("_POSIX_MESSAGE_PASSING", _SC_MESSAGE_PASSING),
    SC("_POSIX_MONOTONIC_CLOCK", _SC_MONOTONIC_CLOCK),
    SC("_POSIX_PRIORITIZED_IO", _SC_PRIORITIZED_IO),
    SC("_POSIX_PRIORITY_SCHEDULING", _SC_PRIORITY_SCHEDULING),
    SC("_POSIX_RAW_SOCKETS", _SC_RAW_SOCKETS),
    SC("_POSIX_READER_WRITER_LOCKS", _SC_READER_WRITER_LOCKS),
    SC("_POSIX_REALTIME_SIGNALS", _SC_REALTIME_SIGNALS),
    SC("_POSIX_REGEXP", _SC_REGEXP),
    SC("_POSIX_SAVED_IDS", _SC_SAVED_IDS),
    SC("_POSIX_SEMAPHORES", _SC_SEMAPHORES),
    SC("_POSIX_SHARED_MEMORY_OBJECTS", _SC_SHARED_MEMORY_OBJECTS),
    SC("_POSIX_SHELL", _SC_SHELL),
    SC("_POSIX_SPAWN", _SC_SPAWN),
    SC("_POSIX_SPIN_LOCKS", _SC_SPIN_LOCKS),
    SC("_POSIX_SPORADIC_SERVER", _SC_SPORADIC_SERVER),
    SC("_POSIX_SS_REPL_MAX", _SC_SS_REPL_MAX),
    SC("_POSIX_SYNCHRONIZED_IO", _SC_SYNCHRONIZED_IO),
    SC("_POSIX_THREAD_ATTR_STACKADDR", _SC_THREAD_ATTR_STACKADDR),
    SC("_POSIX_THREAD_ATTR_STACKSIZE", _SC_THREAD_ATTR_STACKSIZE),
    SC("_POSIX_THREAD_CPUTIME", _SC_THREAD_CPUTIME),
    SC("_POSIX_THREAD_PRIO_INHERIT", _SC_THREAD_PRIO_INHERIT),
    SC("_POSIX_THREAD_PRIO_PROTECT", _SC_THREAD_PRIO_PROTECT),
    SC("_POSIX_THREAD_PRIORITY_SCHEDULING", _SC_THREAD_PRIORITY_SCHEDULING),
    SC("_POSIX_THREAD_PROCESS_SHARED", _SC_THREAD_PROCESS_SHARED),
    SC("_POSIX_THREAD_ROBUST_PRIO_INHERIT", _SC_THREAD_ROBUST_PRIO_INHERIT),
    SC("_POSIX_THREAD_ROBUST_PRIO_PROTECT", _SC_THREAD_ROBUST_PRIO_PROTECT),
    SC("_POSIX_THREAD_SAFE_FUNCTIONS", _SC_THREAD_SAFE_FUNCTIONS),
    SC("_POSIX_THREAD_SPORADIC_SERVER", _SC_THREAD_SPORADIC_SERVER),
    SC("_POSIX_THREADS", _SC_THREADS),
    SC("_POSIX_TIMEOUTS", _SC_TIMEOUTS),
    SC("_POSIX_TIMERS", _SC_TIMERS),
    SC("_POSIX_TYPED_MEMORY_OBJECTS", _SC_TYPED_MEMORY_OBJECTS),
    SC("_POSIX_VERSION", _SC_VERSION),
    SC("_POSIX_V8_ILP32_OFF32", _SC_V8_ILP32_OFF32),
    SC("_POSIX_V8_ILP32_OFFBIG", _SC_V8_ILP32_OFFBIG),
    SC("_POSIX_V8_LP64_OFF64", _SC_V8_LP64_OFF64),
    SC("_POSIX_V8_LPBIG_OFFBIG", _SC_V8_LPBIG_OFFBIG),
    SC("_POSIX_V7_ILP32_OFF32", _SC_V7_ILP32_OFF32),
    SC("_POSIX_V7_ILP32_OFFBIG", _SC_V7_ILP32_OFFBIG),
    SC("_POSIX_V7_LP64_OFF64", _SC_V7_LP64_OFF64),
    SC("_POSIX_V7_LPBIG_OFFBIG", _SC_V7_LPBIG_OFFBIG),
    SC("_POSIX2_C_BIND", _SC_2_C_BIND),
    SC("_POSIX2_C_DEV", _SC_2_C_DEV),
    SC("_POSIX2_CHAR_TERM", _SC_2_CHAR_TERM),
    SC("_POSIX2_FORT_RUN", _SC_2_FORT_RUN),
    SC("_POSIX2_LOCALEDEF", _SC_2_LOCALEDEF),
    SC("_POSIX2_SW_DEV", _SC_2_SW_DEV),
    SC("_POSIX2_UPE", _SC_2_UPE),
    SC("_POSIX2_VERSION", _SC_2_VERSION),
    SC("_XOPEN_CRYPT", _SC_XOPEN_CRYPT),
    SC("_XOPEN_ENH_I18N", _SC_XOPEN_ENH_I18N),
    SC("_XOPEN_REALTIME", _SC_XOPEN_REALTIME),
    SC("_XOPEN_REALTIME_THREADS", _SC_XOPEN_REALTIME_THREADS),
    SC("_XOPEN_SHM", _SC_XOPEN_SHM),
    SC("_XOPEN_UNIX", _SC_XOPEN_UNIX),
    SC("_XOPEN_VERSION", _SC_XOPEN_VERSION),
    /* confstr values */
    CS("PATH", _CS_PATH),
    CS("POSIX_V8_ILP32_OFF32_CFLAGS", _CS_POSIX_V8_ILP32_OFF32_CFLAGS),
    CS("POSIX_V8_ILP32_OFF32_LDFLAGS", _CS_POSIX_V8_ILP32_OFF32_LDFLAGS),
    CS("POSIX_V8_ILP32_OFF32_LIBS", _CS_POSIX_V8_ILP32_OFF32_LIBS),
    CS("POSIX_V8_ILP32_OFFBIG_CFLAGS", _CS_POSIX_V8_ILP32_OFFBIG_CFLAGS),
    CS("POSIX_V8_ILP32_OFFBIG_LDFLAGS", _CS_POSIX_V8_ILP32_OFFBIG_LDFLAGS),
    CS("POSIX_V8_ILP32_OFFBIG_LIBS", _CS_POSIX_V8_ILP32_OFFBIG_LIBS),
    CS("POSIX_V8_LP64_OFF64_CFLAGS", _CS_POSIX_V8_LP64_OFF64_CFLAGS),
    CS("POSIX_V8_LP64_OFF64_LDFLAGS", _CS_POSIX_V8_LP64_OFF64_LDFLAGS),
    CS("POSIX_V8_LP64_OFF64_LIBS", _CS_POSIX_V8_LP64_OFF64_LIBS),
    CS("POSIX_V8_LPBIG_OFFBIG_CFLAGS", _CS_POSIX_V8_LPBIG_OFFBIG_CFLAGS),
    CS("POSIX_V8_LPBIG_OFFBIG_LDFLAGS", _CS_POSIX_V8_LPBIG_OFFBIG_LDFLAGS),
    CS("POSIX_V8_LPBIG_OFFBIG_LIBS", _CS_POSIX_V8_LPBIG_OFFBIG_LIBS),
    CS("POSIX_V8_THREADS_CFLAGS", _CS_POSIX_V8_THREADS_CFLAGS),
    CS("POSIX_V8_THREADS_LDFLAGS", _CS_POSIX_V8_THREADS_LDFLAGS),
    CS("POSIX_V8_WIDTH_RESTRICTED_ENVS", _CS_POSIX_V8_WIDTH_RESTRICTED_ENVS),
    CS("V8_ENV", _CS_V8_ENV),
    CS("POSIX_V7_ILP32_OFF32_CFLAGS", _CS_POSIX_V7_ILP32_OFF32_CFLAGS),
    CS("POSIX_V7_ILP32_OFF32_LDFLAGS", _CS_POSIX_V7_ILP32_OFF32_LDFLAGS),
    CS("POSIX_V7_ILP32_OFF32_LIBS", _CS_POSIX_V7_ILP32_OFF32_LIBS),
    CS("POSIX_V7_ILP32_OFFBIG_CFLAGS", _CS_POSIX_V7_ILP32_OFFBIG_CFLAGS),
    CS("POSIX_V7_ILP32_OFFBIG_LDFLAGS", _CS_POSIX_V7_ILP32_OFFBIG_LDFLAGS),
    CS("POSIX_V7_ILP32_OFFBIG_LIBS", _CS_POSIX_V7_ILP32_OFFBIG_LIBS),
    CS("POSIX_V7_LP64_OFF64_CFLAGS", _CS_POSIX_V7_LP64_OFF64_CFLAGS),
    CS("POSIX_V7_LP64_OFF64_LDFLAGS", _CS_POSIX_V7_LP64_OFF64_LDFLAGS),
    CS("POSIX_V7_LP64_OFF64_LIBS", _CS_POSIX_V7_LP64_OFF64_LIBS),
    CS("POSIX_V7_LPBIG_OFFBIG_CFLAGS", _CS_POSIX_V7_LPBIG_OFFBIG_CFLAGS),
    CS("POSIX_V7_LPBIG_OFFBIG_LDFLAGS", _CS_POSIX_V7_LPBIG_OFFBIG_LDFLAGS),
    CS("POSIX_V7_LPBIG_OFFBIG_LIBS", _CS_POSIX_V7_LPBIG_OFFBIG_LIBS),
    CS("POSIX_V7_THREADS_CFLAGS", _CS_POSIX_V7_THREADS_CFLAGS),
    CS("POSIX_V7_THREADS_LDFLAGS", _CS_POSIX_V7_THREADS_LDFLAGS),
    CS("POSIX_V7_WIDTH_RESTRICTED_ENVS", _CS_POSIX_V7_WIDTH_RESTRICTED_ENVS),
    CS("V7_ENV", _CS_V7_ENV),
    /* <limits.h> maximum and minimum values */
    LIMIT(_POSIX_AIO_LISTIO_MAX), LIMIT(_POSIX_AIO_MAX), LIMIT(_POSIX_ARG_MAX),
    LIMIT(_POSIX_CHILD_MAX), LIMIT(_POSIX_DELAYTIMER_MAX), LIMIT(_POSIX_HOST_NAME_MAX),
    LIMIT(_POSIX_LINK_MAX), LIMIT(_POSIX_LOGIN_NAME_MAX), LIMIT(_POSIX_MAX_CANON),
    LIMIT(_POSIX_MAX_INPUT), LIMIT(_POSIX_MQ_OPEN_MAX), LIMIT(_POSIX_MQ_PRIO_MAX),
    LIMIT(_POSIX_NAME_MAX), LIMIT(_POSIX_NGROUPS_MAX), LIMIT(_POSIX_OPEN_MAX),
    LIMIT(_POSIX_PATH_MAX), LIMIT(_POSIX_PIPE_BUF), LIMIT(_POSIX_RE_DUP_MAX),
    LIMIT(_POSIX_RTSIG_MAX), LIMIT(_POSIX_SEM_NSEMS_MAX), LIMIT(_POSIX_SEM_VALUE_MAX),
    LIMIT(_POSIX_SIGQUEUE_MAX), LIMIT(_POSIX_SSIZE_MAX), LIMIT(_POSIX_SS_REPL_MAX),
    LIMIT(_POSIX_STREAM_MAX), LIMIT(_POSIX_SYMLINK_MAX), LIMIT(_POSIX_SYMLOOP_MAX),
    LIMIT(_POSIX_THREAD_DESTRUCTOR_ITERATIONS), LIMIT(_POSIX_THREAD_KEYS_MAX),
    LIMIT(_POSIX_THREAD_THREADS_MAX), LIMIT(_POSIX_TIMER_MAX), LIMIT(_POSIX_TTY_NAME_MAX),
    LIMIT(_POSIX_TZNAME_MAX), LIMIT(_POSIX2_BC_BASE_MAX), LIMIT(_POSIX2_BC_DIM_MAX),
    LIMIT(_POSIX2_BC_SCALE_MAX), LIMIT(_POSIX2_BC_STRING_MAX),
    LIMIT(_POSIX2_CHARCLASS_NAME_MAX), LIMIT(_POSIX2_COLL_WEIGHTS_MAX),
    LIMIT(_POSIX2_EXPR_NEST_MAX), LIMIT(_POSIX2_LINE_MAX), LIMIT(_POSIX2_RE_DUP_MAX),
    LIMIT(_XOPEN_IOV_MAX), LIMIT(_XOPEN_NAME_MAX), LIMIT(_XOPEN_PATH_MAX),
    LIMIT(CHAR_BIT), LIMIT(CHAR_MAX), LIMIT(CHAR_MIN), LIMIT(CHARCLASS_NAME_MAX),
    LIMIT(INT_MAX), LIMIT(INT_MIN), LIMIT(LLONG_MAX), LIMIT(LLONG_MIN), LIMIT(LONG_BIT),
    LIMIT(LONG_MAX), LIMIT(LONG_MIN), LIMIT(MB_LEN_MAX), LIMIT(NL_ARGMAX), LIMIT(NL_LANGMAX),
    LIMIT(NL_MSGMAX), LIMIT(NL_SETMAX), LIMIT(NL_TEXTMAX), LIMIT(NZERO), LIMIT(SCHAR_MAX),
    LIMIT(SCHAR_MIN), LIMIT(SHRT_MAX), LIMIT(SHRT_MIN), LIMIT(SSIZE_MAX), LIMIT(UCHAR_MAX),
    LIMIT(UINT_MAX), ULIMIT(ULLONG_MAX), ULIMIT(ULONG_MAX), LIMIT(USHRT_MAX), LIMIT(WORD_BIT),
    /* pathconf values */
    PC("FILESIZEBITS", _PC_FILESIZEBITS),
    PC("LINK_MAX", _PC_LINK_MAX),
    PC("MAX_CANON", _PC_MAX_CANON),
    PC("MAX_INPUT", _PC_MAX_INPUT),
    PC("NAME_MAX", _PC_NAME_MAX),
    PC("PATH_MAX", _PC_PATH_MAX),
    PC("PIPE_BUF", _PC_PIPE_BUF),
    PC("POSIX2_SYMLINKS", _PC_2_SYMLINKS),
    PC("POSIX_ALLOC_SIZE_MIN", _PC_ALLOC_SIZE_MIN),
    PC("POSIX_REC_INCR_XFER_SIZE", _PC_REC_INCR_XFER_SIZE),
    PC("POSIX_REC_MAX_XFER_SIZE", _PC_REC_MAX_XFER_SIZE),
    PC("POSIX_REC_MIN_XFER_SIZE", _PC_REC_MIN_XFER_SIZE),
    PC("POSIX_REC_XFER_ALIGN", _PC_REC_XFER_ALIGN),
    PC("SYMLINK_MAX", _PC_SYMLINK_MAX),
    PC("TEXTDOMAIN_MAX", _PC_TEXTDOMAIN_MAX),
    PC("_POSIX_ASYNC_IO", _PC_ASYNC_IO),
    PC("_POSIX_CHOWN_RESTRICTED", _PC_CHOWN_RESTRICTED),
    PC("_POSIX_FALLOC", _PC_FALLOC),
    PC("_POSIX_NO_TRUNC", _PC_NO_TRUNC),
    PC("_POSIX_PRIO_IO", _PC_PRIO_IO),
    PC("_POSIX_SYNC_IO", _PC_SYNC_IO),
    PC("_POSIX_TIMESTAMP_RESOLUTION", _PC_TIMESTAMP_RESOLUTION),
    PC("_POSIX_VDISABLE", _PC_VDISABLE),
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

/* Write a numeric limit: -1 with errno unchanged means no value. */
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

    int status = 0;
    errno = 0;
    switch (var->kind) {
    case SYSCONF:
        status = print_limit(var->name, sysconf((int)var->key));
        break;
    case CONFSTR:
        status = print_string(var->name, (int)var->key);
        break;
    case PATHCONF:
        status = print_limit(var->name, pathconf(argv[1], (int)var->key));
        break;
    case CONSTANT:
        printf("%lld\n", var->key);
        break;
    case UCONSTANT:
        printf("%llu\n", (unsigned long long)var->key);
        break;
    }
    if (fflush(stdout) != 0 || ferror(stdout)) {
        fprintf(stderr, "getconf: write error\n");
        return 1;
    }
    return status;
}
