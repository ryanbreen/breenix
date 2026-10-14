/* libgen.h: POSIX pathname component functions. */
#ifndef _LIBGEN_H
#define _LIBGEN_H

#include <__breenix/features.h>

__BREENIX_BEGIN_DECLS

char *basename(char *);
char *dirname(char *);

__BREENIX_END_DECLS

#endif
