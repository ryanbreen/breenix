#define _POSIX_C_SOURCE 200809L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "app.h"

int cmd_hello(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    printf("hello from c-app\n");
    return 0;
}

int cmd_args(int argc, char **argv)
{
    printf("argc=%d\n", argc);
    for (int i = 1; i < argc; i++) {
        printf("[%s]\n", argv[i]);
    }
    const char *value = getenv("SDK_SUITE_VAR");
    printf("SDK_SUITE_VAR=%s\n", value ? value : "(null)");
    printf("SDK_SUITE_UNSET=%s\n", getenv("SDK_SUITE_UNSET") ? "set" : "(null)");
    return 0;
}

int cmd_exit(int argc, char **argv)
{
    if (argc < 2) {
        printf("exit needs a status\n");
        return 2;
    }
    exit(atoi(argv[1]));
}

int cmd_stdio_format(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    printf("%d|%5d|%-5d|%05d|%x|%X|%o|%s|%.3s|%c|%%\n", -42, 42, 42, 42, 255, 255, 8, "str", "abcdef", 'z');
    printf("%.2f|%e|%g|%g\n", 3.14159, 12345.678, 0.0001, 1e20);
    char buf[5];
    int n = snprintf(buf, sizeof buf, "%s", "abcdefgh");
    printf("snprintf=%d buf=%s\n", n, buf);
    long long big = 1234567890123LL;
    printf("%lld|%zu|%ld\n", big, sizeof(int), -7L);
    return 0;
}

int cmd_stdio_stream(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    FILE *out = fopen("stream.txt", "w");
    if (!out) {
        return failed("fopen w");
    }
    fprintf(out, "line one\n");
    fputs("line two\n", out);
    if (fclose(out) != 0) {
        return failed("fclose");
    }
    FILE *in = fopen("stream.txt", "r");
    if (!in) {
        return failed("fopen r");
    }
    char line[64];
    while (fgets(line, sizeof line, in)) {
        printf("read: %s", line);
    }
    if (fseek(in, 0, SEEK_END) != 0) {
        return failed("fseek");
    }
    printf("size=%ld\n", ftell(in));
    fclose(in);
    if (remove("stream.txt") != 0) {
        return failed("remove");
    }
    return 0;
}

int cmd_stderr(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    fprintf(stderr, "to stderr\n");
    printf("to stdout\n");
    return 0;
}
