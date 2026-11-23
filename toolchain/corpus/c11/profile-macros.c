/* N1570 6.10.8.3p1–2, conditional feature macros. */
#if !defined(__STDC_NO_VLA__) || __STDC_NO_VLA__ != 1
#error expected __STDC_NO_VLA__ to be defined as 1
#endif
#if !defined(__STDC_NO_COMPLEX__) || __STDC_NO_COMPLEX__ != 1
#error expected __STDC_NO_COMPLEX__ to be defined as 1
#endif
#if defined(__STDC_IEC_559_COMPLEX__)
#error expected __STDC_IEC_559_COMPLEX__ to be absent
#endif
#ifdef __STDC_NO_ATOMICS__
#error expected __STDC_NO_ATOMICS__ to be absent
#endif
#if !defined(__STDC_NO_THREADS__) || __STDC_NO_THREADS__ != 1
#error expected __STDC_NO_THREADS__ to be defined as 1
#endif
int profile_macros_verified;
