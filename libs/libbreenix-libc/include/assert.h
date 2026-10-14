/*
 * assert.h: diagnostics (ISO C 7.2). Each inclusion defines assert by whether NDEBUG
 * is defined at that point, so the header has no include guard.
 */
#include <__breenix/features.h>

#undef assert
#ifdef NDEBUG
#define assert(ignore) ((void)0)
#else
__BREENIX_BEGIN_DECLS
/* Writes "<file>:<line>: <function>: assertion failed: <expression>" to standard error,
 * then calls abort. */
__BREENIX_NORETURN void __breenix_assert_fail(const char *, const char *, int, const char *);
__BREENIX_END_DECLS
#define assert(e) ((e) ? (void)0 : __breenix_assert_fail(#e, __FILE__, __LINE__, __func__))
#endif

#if !defined(__cplusplus) && defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L && \
    __STDC_VERSION__ < 202311L && !defined(static_assert)
#define static_assert _Static_assert
#endif
