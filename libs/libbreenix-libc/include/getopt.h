/*
 * POSIX.1-2024 getopt and getsubopt, plus Breenix long-option extensions.
 * struct option describes a name, argument policy, optional result location,
 * and result value. Argument policies are our ordered enum: none=0, required=1,
 * optional=2.
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
