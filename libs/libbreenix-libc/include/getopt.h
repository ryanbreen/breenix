/*
 * <getopt.h>: getopt_long and getopt_long_only with Linux's struct option (getopt(3)),
 * plus the POSIX getopt declarations that <unistd.h> also carries.
 */
#ifndef _GETOPT_H
#define _GETOPT_H

#include <__breenix/features.h>

__BREENIX_BEGIN_DECLS

extern char *optarg;
extern int optind, opterr, optopt;

int getopt(int, char *const[], const char *);

struct option {
    const char *name;
    int has_arg;
    int *flag;
    int val;
};

#define no_argument 0
#define required_argument 1
#define optional_argument 2

int getopt_long(int, char *const[], const char *, const struct option *, int *);
int getopt_long_only(int, char *const[], const char *, const struct option *, int *);

__BREENIX_END_DECLS

#endif
