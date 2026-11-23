/* C17 corpus: corrections.
 * One check for each WG14 N2244 report that 25.10a1 ledgered in
 * toolchain/docs/spec-ledger.md, each printing one line named for its report.
 * The C17 paragraphs are cited as the N2244 corrigenda change them.
 * Clauses: N2244 DR 412, DR 423, DR 444, DR 465, DR 481;
 *   C17 6.10.1p6, 6.5.4p5, 6.7.6.3p5, 6.7.2.1p1, 6.7.5p2, 7.17.5.1p2-3, 6.5.1.1p2;
 *   N1570 6.2.8p1, 6.2.8p4, 6.3.2.1p2-4, 6.5.1.1p3, 6.7.2.1p15, 6.7.5p6, 7.1.4p1, 7.17.5p1, 7.19p3.
 * Expectations: corrections.expect, written by hand from these clauses.
 */
#include <stdatomic.h>
#include <stddef.h>
#include <stdio.h>

/* DR 412: once a group is taken, every later #elif is processed as if it
 * were in a skipped group, so its controlling expression is never evaluated
 * (C17 6.10.1p6). Both trailing #elif lines are not expressions; the first is
 * the report's own example. */
#if 1
#define DR412_FIRST 1
#elif this is not a valid expression
#define DR412_FIRST 2
#endif

#if 0
#define DR412_SECOND 1
#elif 2 > 1
#define DR412_SECOND 2
#elif ) not ( an expression
#define DR412_SECOND 3
#endif

/* DR 423: a function declared with a qualified result type returns the
 * unqualified version of that type (C17 6.7.6.3p5), so dr423_result has type
 * int (void) and its designator converts to int (*)(void). A cast converts to
 * the unqualified version of the named type (C17 6.5.4p5). */
typedef const int dr423_const_int;

static dr423_const_int dr423_result(void)
{
    return 7;
}

static void dr423(void)
{
    int function = _Generic(dr423_result, int (*)(void): 1, default: 0);
    int cast = _Generic((const int)5, int: 1, default: 0);
    printf("DR 423: %d %d %d\n", function, cast, dr423_result());
}

/* DR 444: an alignment specifier may appear in a member declaration
 * (C17 6.7.2.1p1, 6.7.5p2), and the member's alignment requirement is the
 * specified one (N1570 6.7.5p6). Alignments are powers of two (6.2.8p4), so
 * every placement of the structure puts the member at a multiple of 8: the
 * structure's alignment, its size and the member offset are multiples of 8,
 * and the member follows the first member, which is at offset 0 (6.7.2.1p15). */
struct dr444 {
    char first;
    _Alignas(8) char aligned;
    _Alignas(double) char typed;
};

static void dr444(void)
{
    printf("DR 444: %d %d %d %d %d\n",
           (int)(offsetof(struct dr444, aligned) % 8),
           (int)(_Alignof(struct dr444) % 8),
           (int)(sizeof(struct dr444) % 8),
           offsetof(struct dr444, aligned) >= 8,
           (int)(offsetof(struct dr444, typed) % _Alignof(double)));
}

/* DR 465: atomic_is_lock_free answers for the type its argument points to,
 * the same for every pointer of that type, and the argument may be a null
 * pointer (C17 7.17.5.1p2-3). ATOMIC_INT_LOCK_FREE 2 means always lock-free
 * and 0 never (N1570 7.17.5p1). The argument is evaluated exactly once
 * (7.1.4p1). */
static _Atomic int dr465_object;
static int dr465_calls;

static _Atomic int *dr465_null(void)
{
    ++dr465_calls;
    return NULL;
}

static void dr465(void)
{
    _Atomic int *none = NULL;
    _Bool on_null = atomic_is_lock_free(none);
    _Bool on_object = atomic_is_lock_free(&dr465_object);
    int agrees = ATOMIC_INT_LOCK_FREE == 1 || on_null == (ATOMIC_INT_LOCK_FREE == 2);
    (void)atomic_is_lock_free(dr465_null());
    printf("DR 465: %d %d %d\n", on_null == on_object, agrees, dr465_calls);
}

/* DR 481: the type of a generic selection's controlling expression is its
 * type after lvalue conversion, which drops qualifiers, or after array-to-
 * pointer or function-to-pointer conversion (C17 6.5.1.1p2; N1570
 * 6.3.2.1p2-4). The controlling expression is not evaluated (6.5.1.1p3). */
static int dr481_function(void)
{
    return 0;
}

static void dr481(void)
{
    const int qualified = 4;
    char array[3] = { 0 };
    int (*pointer)(void) = dr481_function;
    int lvalue = _Generic(qualified, int: 1, default: 0);
    int decayed = _Generic(array, char *: 1, default: 0);
    int function = _Generic(dr481_function, int (*)(void): 1, default: 0);
    int zero = 0;
    /* Evaluating 1 / zero would be undefined; the selection never does. */
    int unevaluated = _Generic(1 / zero, int: 1, default: 0);
    printf("DR 481: %d %d %d %d %d\n", lvalue, decayed, function, unevaluated, pointer());
}

int main(void)
{
    printf("DR 412: %d %d\n", DR412_FIRST, DR412_SECOND);
    dr423();
    dr444();
    dr465();
    dr481();
    return 0;
}
