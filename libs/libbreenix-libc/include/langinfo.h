#ifndef __BREENIX_LANGINFO_H
#define __BREENIX_LANGINFO_H
#include <__breenix/types.h>
#include <__breenix/features.h>
typedef int nl_item;
typedef __breenix_locale_t locale_t;
/*
 * Breenix numbering: the items are numbered consecutively from 1 in the order the
 * POSIX.1-2024 <langinfo.h> table lists them. src/locale/objects.rs mirrors these
 * values, and its host test checks this header against them.
 */
#define CODESET 1
#define D_T_FMT 2
#define D_FMT 3
#define T_FMT 4
#define T_FMT_AMPM 5
#define AM_STR 6
#define PM_STR 7
#define DAY_1 8
#define DAY_2 9
#define DAY_3 10
#define DAY_4 11
#define DAY_5 12
#define DAY_6 13
#define DAY_7 14
#define ABDAY_1 15
#define ABDAY_2 16
#define ABDAY_3 17
#define ABDAY_4 18
#define ABDAY_5 19
#define ABDAY_6 20
#define ABDAY_7 21
#define MON_1 22
#define MON_2 23
#define MON_3 24
#define MON_4 25
#define MON_5 26
#define MON_6 27
#define MON_7 28
#define MON_8 29
#define MON_9 30
#define MON_10 31
#define MON_11 32
#define MON_12 33
#define ALTMON_1 34
#define ALTMON_2 35
#define ALTMON_3 36
#define ALTMON_4 37
#define ALTMON_5 38
#define ALTMON_6 39
#define ALTMON_7 40
#define ALTMON_8 41
#define ALTMON_9 42
#define ALTMON_10 43
#define ALTMON_11 44
#define ALTMON_12 45
#define ABMON_1 46
#define ABMON_2 47
#define ABMON_3 48
#define ABMON_4 49
#define ABMON_5 50
#define ABMON_6 51
#define ABMON_7 52
#define ABMON_8 53
#define ABMON_9 54
#define ABMON_10 55
#define ABMON_11 56
#define ABMON_12 57
#define ABALTMON_1 58
#define ABALTMON_2 59
#define ABALTMON_3 60
#define ABALTMON_4 61
#define ABALTMON_5 62
#define ABALTMON_6 63
#define ABALTMON_7 64
#define ABALTMON_8 65
#define ABALTMON_9 66
#define ABALTMON_10 67
#define ABALTMON_11 68
#define ABALTMON_12 69
#define ERA 70
#define ERA_D_FMT 71
#define ERA_D_T_FMT 72
#define ERA_T_FMT 73
#define ALT_DIGITS 74
#define RADIXCHAR 75
#define THOUSEP 76
#define YESEXPR 77
#define NOEXPR 78
#define CRNCYSTR 79
char *nl_langinfo(nl_item);
char *nl_langinfo_l(nl_item, locale_t);
#endif
