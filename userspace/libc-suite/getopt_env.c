/*
 * libc suite, getopt-env category: getopt (<unistd.h>), getopt_long (<getopt.h>, Linux's
 * ABI), getsubopt, environ, getenv, setenv, unsetenv and putenv. Every case runs in a new
 * process, so getopt starts each one with optind 1. The suite runs the program with the
 * environment PATH=/usr/local/libc/bin, HOME=/tmp, LIBC_SUITE=1 and nothing else.
 */
#define _POSIX_C_SOURCE 202405L
#include "case.h"

#include <errno.h>
#include <fcntl.h>
#include <getopt.h>
#include <stdlib.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

#define ARGC(v) ((int)(sizeof(v) / sizeof((v)[0])) - 1)

/* Send stderr to a pipe whose read end does not block; the read end or -1. */
static int capture_stderr(void) {
    int p[2];
    if (pipe(p) != 0 || fcntl(p[0], F_SETFL, O_NONBLOCK) != 0 || dup2(p[1], 2) != 2)
        return -1;
    return p[0];
}

static int contains(const char *s, const char *part) {
    size_t n = h_strlen(part);
    for (; *s; s++)
        if (h_memeq(s, part, n))
            return 1;
    return 0;
}

static int env_count(const char *prefix) {
    int n = 0;
    size_t len = h_strlen(prefix);
    for (char **e = environ; *e; e++)
        if (h_memeq(*e, prefix, len))
            n++;
    return n;
}

static const char *getopt_initial(void) {
    EXPECT_INT("optind before the first getopt", optind, 1);
    EXPECT_INT("opterr before the first getopt", opterr, 1);
    return PASS;
}

static const char *getopt_flags(void) {
    char *v[] = { "p", "-a", "-b", "x", NULL };
    EXPECT_INT("first getopt", getopt(ARGC(v), v, "ab"), 'a');
    EXPECT_INT("second getopt", getopt(ARGC(v), v, "ab"), 'b');
    EXPECT_INT("getopt at the operand", getopt(ARGC(v), v, "ab"), -1);
    EXPECT_INT("optind at the operand", optind, 3);
    return PASS;
}

static const char *getopt_attached_arg(void) {
    char *v[] = { "p", "-ofile", NULL };
    EXPECT_INT("getopt", getopt(ARGC(v), v, "o:"), 'o');
    EXPECT_STR("optarg", optarg, "file");
    EXPECT_INT("getopt at the end", getopt(ARGC(v), v, "o:"), -1);
    EXPECT_INT("optind at the end", optind, 2);
    return PASS;
}

static const char *getopt_separate_arg(void) {
    char *v[] = { "p", "-o", "file", "rest", NULL };
    EXPECT_INT("getopt", getopt(ARGC(v), v, "o:"), 'o');
    EXPECT_STR("optarg", optarg, "file");
    EXPECT_INT("getopt at the operand", getopt(ARGC(v), v, "o:"), -1);
    EXPECT_INT("optind at the operand", optind, 3);
    return PASS;
}

static const char *getopt_grouped(void) {
    char *v[] = { "p", "-abc", NULL };
    EXPECT_INT("first", getopt(ARGC(v), v, "abc"), 'a');
    EXPECT_INT("optind inside a group", optind, 1);
    EXPECT_INT("second", getopt(ARGC(v), v, "abc"), 'b');
    EXPECT_INT("third", getopt(ARGC(v), v, "abc"), 'c');
    EXPECT_INT("optind after the group", optind, 2);
    EXPECT_INT("getopt at the end", getopt(ARGC(v), v, "abc"), -1);
    return PASS;
}

static const char *getopt_grouped_arg(void) {
    char *v[] = { "p", "-acfoo", NULL };
    EXPECT_INT("first", getopt(ARGC(v), v, "ac:"), 'a');
    EXPECT_INT("second", getopt(ARGC(v), v, "ac:"), 'c');
    EXPECT_STR("optarg from the rest of the group", optarg, "foo");
    EXPECT_INT("optind", optind, 2);
    return PASS;
}

static const char *getopt_double_dash(void) {
    char *v[] = { "p", "-a", "--", "-b", NULL };
    EXPECT_INT("first", getopt(ARGC(v), v, "ab"), 'a');
    EXPECT_INT("getopt at \"--\"", getopt(ARGC(v), v, "ab"), -1);
    EXPECT_INT("optind after \"--\"", optind, 3);
    return PASS;
}

static const char *getopt_stops_at_operand(void) {
    char *v[] = { "p", "x", "-a", NULL };
    EXPECT_INT("getopt at a leading operand", getopt(ARGC(v), v, "a"), -1);
    EXPECT_INT("optind", optind, 1);
    EXPECT_STR("argv[1] is not moved", v[1], "x");
    return PASS;
}

static const char *getopt_lone_dash(void) {
    char *v[] = { "p", "-", "-a", NULL };
    EXPECT_INT("getopt at \"-\"", getopt(ARGC(v), v, "a"), -1);
    EXPECT_INT("optind", optind, 1);
    return PASS;
}

static const char *getopt_unknown(void) {
    char *v[] = { "p", "-z", NULL };
    char buf[128];
    int err = capture_stderr();
    CHECK(err >= 0, "could not capture stderr");
    opterr = 0;
    EXPECT_INT("getopt of an unknown option", getopt(ARGC(v), v, "a"), '?');
    EXPECT_INT("optopt", optopt, 'z');
    h_read_now(err, buf, sizeof buf);
    EXPECT_STR("stderr with opterr 0", buf, "");
    return PASS;
}

static const char *getopt_diagnostic(void) {
    char *v[] = { "prog", "-z", NULL };
    char buf[256];
    int err = capture_stderr();
    CHECK(err >= 0, "could not capture stderr");
    EXPECT_INT("getopt of an unknown option", getopt(ARGC(v), v, "a"), '?');
    h_read_now(err, buf, sizeof buf);
    CHECK(buf[0] != 0, "getopt wrote no diagnostic to stderr");
    CHECK(contains(buf, "z"), "the diagnostic does not name the option");
    return PASS;
}

static const char *getopt_missing_arg(void) {
    char *v[] = { "p", "-o", NULL };
    EXPECT_INT("getopt of an option missing its argument", getopt(ARGC(v), v, "o:"), '?');
    EXPECT_INT("optopt", optopt, 'o');
    return PASS;
}

static const char *getopt_missing_colon(void) {
    char *v[] = { "p", "-o", NULL };
    char buf[128];
    int err = capture_stderr();
    CHECK(err >= 0, "could not capture stderr");
    EXPECT_INT("getopt with a leading ':' in optstring", getopt(ARGC(v), v, ":o:"), ':');
    EXPECT_INT("optopt", optopt, 'o');
    h_read_now(err, buf, sizeof buf);
    EXPECT_STR("stderr with a leading ':'", buf, "");
    return PASS;
}

static const char *getopt_dash_argument(void) {
    char *v[] = { "p", "-o", "-a", NULL };
    EXPECT_INT("getopt", getopt(ARGC(v), v, "ao:"), 'o');
    EXPECT_STR("an option-argument that starts with '-'", optarg, "-a");
    EXPECT_INT("getopt at the end", getopt(ARGC(v), v, "ao:"), -1);
    return PASS;
}

static int flag_value;

static const struct option long_opts[] = {
    { "verbose", no_argument, NULL, 'v' },
    { "version", no_argument, NULL, 'V' },
    { "file", required_argument, NULL, 'f' },
    { "color", optional_argument, NULL, 'c' },
    { "quiet", no_argument, &flag_value, 7 },
    { NULL, 0, NULL, 0 },
};

static const char *long_basic(void) {
    char *v[] = { "p", "--verbose", NULL };
    EXPECT_INT("getopt_long(\"--verbose\")", getopt_long(ARGC(v), v, "", long_opts, NULL), 'v');
    EXPECT_INT("getopt_long at the end", getopt_long(ARGC(v), v, "", long_opts, NULL), -1);
    EXPECT_INT("optind", optind, 2);
    return PASS;
}

static const char *long_required(void) {
    char *v[] = { "p", "--file=x", "--file", "y", NULL };
    EXPECT_INT("--file=x", getopt_long(ARGC(v), v, "", long_opts, NULL), 'f');
    EXPECT_STR("optarg of --file=x", optarg, "x");
    EXPECT_INT("--file y", getopt_long(ARGC(v), v, "", long_opts, NULL), 'f');
    EXPECT_STR("optarg of --file y", optarg, "y");
    EXPECT_INT("at the end", getopt_long(ARGC(v), v, "", long_opts, NULL), -1);
    EXPECT_INT("optind", optind, 4);
    return PASS;
}

static const char *long_flag(void) {
    char *v[] = { "p", "--quiet", NULL };
    EXPECT_INT("getopt_long of an option with a flag", getopt_long(ARGC(v), v, "", long_opts, NULL), 0);
    EXPECT_INT("the flag", flag_value, 7);
    return PASS;
}

static const char *long_index(void) {
    char *v[] = { "p", "--color", NULL };
    int index = -1;
    EXPECT_INT("getopt_long", getopt_long(ARGC(v), v, "", long_opts, &index), 'c');
    EXPECT_INT("longindex", index, 3);
    return PASS;
}

static const char *long_abbrev(void) {
    char *v[] = { "p", "--verb", "--fi", "z", NULL };
    EXPECT_INT("getopt_long of the unique prefix --verb", getopt_long(ARGC(v), v, "", long_opts, NULL), 'v');
    EXPECT_INT("getopt_long of the unique prefix --fi", getopt_long(ARGC(v), v, "", long_opts, NULL), 'f');
    EXPECT_STR("optarg", optarg, "z");
    return PASS;
}

static const char *long_ambiguous(void) {
    char *v[] = { "p", "--ver", NULL };
    opterr = 0;
    EXPECT_INT("getopt_long of the ambiguous --ver", getopt_long(ARGC(v), v, "", long_opts, NULL), '?');
    return PASS;
}

static const char *long_optional(void) {
    char *v[] = { "p", "--color", "--color=always", NULL };
    EXPECT_INT("--color", getopt_long(ARGC(v), v, "", long_opts, NULL), 'c');
    EXPECT_PTR("optarg of --color", optarg, NULL);
    EXPECT_INT("--color=always", getopt_long(ARGC(v), v, "", long_opts, NULL), 'c');
    EXPECT_STR("optarg of --color=always", optarg, "always");
    return PASS;
}

static const char *long_unknown(void) {
    char *v[] = { "p", "--nope", NULL };
    opterr = 0;
    EXPECT_INT("getopt_long of an unknown option", getopt_long(ARGC(v), v, "", long_opts, NULL), '?');
    return PASS;
}

static const char *long_missing(void) {
    char *v[] = { "p", "--file", NULL };
    opterr = 0;
    EXPECT_INT("getopt_long of --file without its argument", getopt_long(ARGC(v), v, ":", long_opts, NULL), ':');
    return PASS;
}

static const char *long_mixed_short(void) {
    char *v[] = { "p", "-v", "--file", "z", "-q", NULL };
    EXPECT_INT("-v", getopt_long(ARGC(v), v, "vqf:", long_opts, NULL), 'v');
    EXPECT_INT("--file z", getopt_long(ARGC(v), v, "vqf:", long_opts, NULL), 'f');
    EXPECT_STR("optarg", optarg, "z");
    EXPECT_INT("-q", getopt_long(ARGC(v), v, "vqf:", long_opts, NULL), 'q');
    EXPECT_INT("at the end", getopt_long(ARGC(v), v, "vqf:", long_opts, NULL), -1);
    return PASS;
}

static const char *getsubopt_tokens(void) {
    char *const tokens[] = { "ro", "size", NULL };
    char opts[] = "ro,size=10,bogus";
    char *p = opts, *value = (char *)1;
    EXPECT_INT("getsubopt of \"ro\"", getsubopt(&p, tokens, &value), 0);
    EXPECT_PTR("value of \"ro\"", value, NULL);
    EXPECT_INT("getsubopt of \"size=10\"", getsubopt(&p, tokens, &value), 1);
    EXPECT_STR("value of \"size=10\"", value, "10");
    EXPECT_INT("getsubopt of an unknown token", getsubopt(&p, tokens, &value), -1);
    EXPECT_STR("value of an unknown token", value, "bogus");
    EXPECT_INT("rest after the last token", *p, 0);
    return PASS;
}

static const char *environ_startup(void) {
    EXPECT_PTR("environ at startup against main's envp", environ, libc_case_envp);
    int n = 0;
    for (char **e = environ; *e; e++)
        n++;
    EXPECT_INT("environment entries", n, 3);
    EXPECT_INT("entries LIBC_SUITE=1", env_count("LIBC_SUITE=1"), 1);
    EXPECT_INT("entries HOME=/tmp", env_count("HOME=/tmp"), 1);
    return PASS;
}

static const char *getenv_lookup(void) {
    EXPECT_STR("getenv(\"LIBC_SUITE\")", getenv("LIBC_SUITE"), "1");
    EXPECT_STR("getenv(\"HOME\")", getenv("HOME"), "/tmp");
    EXPECT_PTR("getenv of a prefix of a name", getenv("LIBC"), NULL);
    EXPECT_PTR("getenv of a missing name", getenv("NOPE"), NULL);
    return PASS;
}

static char putenv_buf[] = "PV=1";

static const char *putenv_shares(void) {
    EXPECT_INT("putenv(\"PV=1\")", putenv(putenv_buf), 0);
    EXPECT_STR("getenv(\"PV\")", getenv("PV"), "1");
    putenv_buf[3] = '2';
    EXPECT_STR("getenv(\"PV\") after changing the string given to putenv", getenv("PV"), "2");
    return PASS;
}

static const char *putenv_replaces(void) {
    static char a[] = "XV=1", b[] = "XV=2";
    EXPECT_INT("putenv(\"XV=1\")", putenv(a), 0);
    EXPECT_INT("putenv(\"XV=2\")", putenv(b), 0);
    EXPECT_STR("getenv(\"XV\")", getenv("XV"), "2");
    EXPECT_INT("entries named XV", env_count("XV="), 1);
    return PASS;
}

static const char *setenv_environ(void) {
    EXPECT_INT("setenv(\"NEWV\", \"abc\", 1)", setenv("NEWV", "abc", 1), 0);
    EXPECT_INT("environ entries NEWV=abc", env_count("NEWV=abc"), 1);
    EXPECT_INT("unsetenv(\"NEWV\")", unsetenv("NEWV"), 0);
    EXPECT_INT("environ entries named NEWV after unsetenv", env_count("NEWV="), 0);
    EXPECT_INT("unsetenv of a missing name", unsetenv("NOPE"), 0);
    errno = 0;
    EXPECT_RET_ERRNO("unsetenv(\"\")", unsetenv(""), -1, EINVAL);
    errno = 0;
    EXPECT_RET_ERRNO("unsetenv(\"A=B\")", unsetenv("A=B"), -1, EINVAL);
    return PASS;
}

static const char *env_fork_inherits(void) {
    CHECK(setenv("FORKV", "42", 1) == 0, "setenv failed");
    pid_t pid = fork();
    CHECK(pid >= 0, "fork failed");
    if (pid == 0) {
        const char *v = getenv("FORKV");
        _exit(v && h_streq(v, "42") ? 0 : 1);
    }
    int status = 0;
    CHECK(waitpid(pid, &status, 0) == pid, "waitpid failed");
    CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0, "the child did not see FORKV=42");
    return PASS;
}

const struct libc_case libc_cases[] = {
    { "getopt-initial", getopt_initial },
    { "getopt-flags", getopt_flags },
    { "getopt-attached-arg", getopt_attached_arg },
    { "getopt-separate-arg", getopt_separate_arg },
    { "getopt-grouped", getopt_grouped },
    { "getopt-grouped-arg", getopt_grouped_arg },
    { "getopt-double-dash", getopt_double_dash },
    { "getopt-stops-at-operand", getopt_stops_at_operand },
    { "getopt-lone-dash", getopt_lone_dash },
    { "getopt-unknown", getopt_unknown },
    { "getopt-diagnostic", getopt_diagnostic },
    { "getopt-missing-arg", getopt_missing_arg },
    { "getopt-missing-colon", getopt_missing_colon },
    { "getopt-dash-argument", getopt_dash_argument },
    { "long-basic", long_basic },
    { "long-required", long_required },
    { "long-flag", long_flag },
    { "long-index", long_index },
    { "long-abbrev", long_abbrev },
    { "long-ambiguous", long_ambiguous },
    { "long-optional", long_optional },
    { "long-unknown", long_unknown },
    { "long-missing", long_missing },
    { "long-mixed-short", long_mixed_short },
    { "getsubopt-tokens", getsubopt_tokens },
    { "environ-startup", environ_startup },
    { "getenv-lookup", getenv_lookup },
    { "putenv-shares", putenv_shares },
    { "putenv-replaces", putenv_replaces },
    { "setenv-environ", setenv_environ },
    { "env-fork-inherits", env_fork_inherits },
    { 0, 0 },
};
