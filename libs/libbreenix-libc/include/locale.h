#ifndef __BREENIX_LOCALE_H
#define __BREENIX_LOCALE_H
#include <__breenix/types.h>
#include <__breenix/features.h>
#define __need_NULL
#include <stddef.h>
/*
 * Breenix numbering: the categories are numbered 0-6 in the order POSIX.1-2024
 * <locale.h> lists them (LC_ALL first), and each LC_*_MASK is 1 << its category,
 * so LC_ALL_MASK is the union of bits 1-6. src/locale/objects.rs mirrors these values.
 */
#define LC_ALL 0
#define LC_COLLATE 1
#define LC_CTYPE 2
#define LC_MESSAGES 3
#define LC_MONETARY 4
#define LC_NUMERIC 5
#define LC_TIME 6
struct lconv {
    char *decimal_point, *thousands_sep, *grouping, *int_curr_symbol, *currency_symbol;
    char *mon_decimal_point, *mon_thousands_sep, *mon_grouping, *positive_sign, *negative_sign;
    char int_frac_digits, frac_digits, p_cs_precedes, p_sep_by_space, n_cs_precedes, n_sep_by_space;
    char p_sign_posn, n_sign_posn, int_p_cs_precedes, int_p_sep_by_space, int_n_cs_precedes;
    char int_n_sep_by_space, int_p_sign_posn, int_n_sign_posn;
};
char *setlocale(int, const char *);
struct lconv *localeconv(void);
#if defined(__BREENIX_POSIX)
typedef __breenix_locale_t locale_t;
#define LC_GLOBAL_LOCALE ((locale_t)-1L)
#define LC_COLLATE_MASK (1 << LC_COLLATE)
#define LC_CTYPE_MASK (1 << LC_CTYPE)
#define LC_MESSAGES_MASK (1 << LC_MESSAGES)
#define LC_MONETARY_MASK (1 << LC_MONETARY)
#define LC_NUMERIC_MASK (1 << LC_NUMERIC)
#define LC_TIME_MASK (1 << LC_TIME)
#define LC_ALL_MASK (LC_COLLATE_MASK | LC_CTYPE_MASK | LC_MESSAGES_MASK | LC_MONETARY_MASK | \
                     LC_NUMERIC_MASK | LC_TIME_MASK)
locale_t newlocale(int, const char *, locale_t);
locale_t duplocale(locale_t);
void freelocale(locale_t);
locale_t uselocale(locale_t);
const char *getlocalename_l(int, locale_t);
#endif
#endif
