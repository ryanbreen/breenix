/* alloca.h: stack allocation, provided by the compiler. */
#ifndef _ALLOCA_H
#define _ALLOCA_H

#define __need_size_t
#include <stddef.h>

#define alloca(size) __builtin_alloca(size)

#endif
