/* <malloc.h>: the GNU allocation header, kept for programs that include it. */
#ifndef __BREENIX_MALLOC_H
#define __BREENIX_MALLOC_H

#include <__breenix/features.h>
#define __need_size_t
#define __need_NULL
#include <stddef.h>

__BREENIX_BEGIN_DECLS

void *malloc(size_t size);
void *calloc(size_t n, size_t size);
void *realloc(void *p, size_t size);
void free(void *p);
void *memalign(size_t alignment, size_t size);
size_t malloc_usable_size(void *p);

__BREENIX_END_DECLS

#endif
