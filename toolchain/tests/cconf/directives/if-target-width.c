#if 0x7FFFFFFFFFFFFFFF > 0
intmax_is_64_bits
#endif
#if 2147483647 + 1 > 0
no_int_width_overflow
#endif
#if 4294967295 + 1 == 4294967296
no_32_bit_wrap
#endif
#if -1 < 0u
WRONG_signed_compare
#else
minus_one_converts_to_uintmax
#endif
#if 0xFFFFFFFFFFFFFFFF == -1
hex_max_is_uintmax
#endif
#if 0x8000000000000000 > 0
hex_top_bit_is_unsigned
#endif
#if (1 ? -1 : 0u) > 0
conditional_converts_to_unsigned
#endif
#if 1u << 63 == 0x8000000000000000
unsigned_shift_63
#endif
#if -9223372036854775807 - 1 < 0
intmax_min
#endif
#if 0 && (1 / 0)
WRONG_unevaluated
#else
unevaluated_division_by_zero
#endif
#if UNDEFINED_NAME == 0 && int == 0 && true == 0
identifiers_become_zero
#endif
#if defined UNDEFINED_NAME || !defined(__STDC__)
WRONG_defined
#elif defined(__STDC__) && __STDC__ == 1
defined_forms
#endif
#if 0
#bogus directive in a skipped group
#error not reached
#if nested
#endif
WRONG_skipped
#elif 1
elif_taken
#else
WRONG_else
#endif
