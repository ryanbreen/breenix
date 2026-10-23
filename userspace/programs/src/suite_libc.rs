//! The C library: Breenix's own clean-room libc (libs/libbreenix-libc) measured through
//! C programs compiled against it, as ISO C17 and POSIX.1-2024 specify it, with Linux's
//! ABI where POSIX leaves a choice open.
//!
//! A non-BusyBox case is a C function in userspace/libc-suite/<category>.c.
//! Each category builds into one program, /usr/libexec/breenix/libc-<category>_test, that
//! this suite runs once per case as `libc-<category>_test <case> <fd>` in a scratch
//! directory of its own, with the environment PATH=/usr/libexec/breenix/libc/bin, HOME=/tmp,
//! LIBC_SUITE=1. The program writes `PASS`, `FAIL <why>` or `SKIP <why>` to descriptor
//! <fd>; case.h describes the contract. Until a category's program is built against
//! Breenix's libc and installed, each of its cases fails saying so.
//!
//! The busybox cases run applets of a BusyBox built against Breenix's libc, installed as
//! /usr/libexec/breenix/libc/bin/busybox with its applets linked beside it. The /bin BusyBox is the same
//! clean-room-libc build; the suite uses its own applet-link directory on PATH.
//! Until that BusyBox is installed, each busybox case fails saying so; an applet the
//! build left out fails with what BusyBox printed.
//!
//! Each case runs in its own forked child under the runner's default 10-second limit.
//! The program it starts gets until 1.5 seconds before that limit; one still running
//! then is killed and the case fails saying it did not finish.
use libbreenix::fs;
use libbreenix::io::{self, fcntl_cmd, poll_events, status_flags, PollFd};
use libbreenix::process;
use libbreenix::signal;
use breenix_suite::suite::{case, case_ms_left, category, fail, running_case, suite, CaseError, CaseResult, Suite};
use libbreenix::time;
use libbreenix::types::Fd;
use libbreenix::error::Error;
use libbreenix::Errno;
use std::ffi::CString;

/// Where the C case programs are installed.
const CASE_DIR: &str = "/usr/libexec/breenix";
/// BusyBox built against Breenix's libc.
const BUSYBOX: &str = "/usr/libexec/breenix/libc/bin/busybox";
/// The whole environment every program the suite starts receives.
const ENV: &[&str] = &["PATH=/usr/libexec/breenix/libc/bin", "HOME=/tmp", "LIBC_SUITE=1"];
/// An argument replaced by the number of the descriptor the C program reports on.
const RESULT_FD: &str = "{result-fd}";
/// How much of the case's limit is kept back for killing and reporting a program that hangs.
const RESERVE_MS: u64 = 1500;

static SUITE: Suite = suite("libc", "C library", &[
    category("stdio", "stdio streams", &[
        case("fopen-missing", "fopen of a missing file in mode r returns NULL with errno ENOENT", c_case),
        case("fopen-w-truncates", "fopen mode w truncates an existing file to length 0", c_case),
        case("fopen-a-appends", "fopen mode a writes at the end of file even after fseek to the start", c_case),
        case("fopen-rplus-update", "fopen mode r+ updates in place without truncating, with fseek between read and write", c_case),
        case("fopen-x-exclusive", "fopen mode wx fails with EEXIST on an existing file and creates a new one", c_case),
        case("fopen-create-mode", "fopen creates a file with mode 0666 less the umask (0644 under umask 022)", c_case),
        case("setvbuf-full", "A fully buffered stream (_IOFBF) holds output until fflush", c_case),
        case("setvbuf-line", "A line-buffered stream (_IOLBF) writes when a newline is written", c_case),
        case("setvbuf-none", "An unbuffered stream (_IONBF) writes each byte at once", c_case),
        case("fflush-all", "fflush(NULL) flushes every output stream", c_case),
        case("fseek-ftell", "fseek with SEEK_SET, SEEK_END and SEEK_CUR moves the position ftell reports, and rewind resets it", c_case),
        case("fseek-einval", "fseek to a negative offset fails with EINVAL and leaves the position unchanged", c_case),
        case("ungetc-pushback", "ungetc pushes back a byte the next read returns without changing the file, and ungetc(EOF) fails", c_case),
        case("ungetc-clears-eof", "ungetc clears the end-of-file indicator", c_case),
        case("printf-integers", "printf %d %i %u %o %x %X and the # flag format integers, and the return is the length", c_case),
        case("printf-flags", "printf field widths, precisions and the - 0 + space flags pad integers as C17 7.21.6.1 specifies", c_case),
        case("printf-strings", "printf %c, %s with precision and width, and %% format as specified", c_case),
        case("printf-floats", "printf %f %e %g %G with precisions and the # flag, rounding 2.5 to even under %.0f", c_case),
        case("printf-length", "printf length modifiers l ll h hh z j t convert their arguments to the named type", c_case),
        case("printf-inf-nan", "printf writes inf/infinity and nan for infinities and NaNs, honouring the field width", c_case),
        case("snprintf-truncate", "snprintf truncates to the buffer size, always terminates, and returns the untruncated length", c_case),
        case("scanf-integers", "sscanf %d %x %o %i read decimal, hexadecimal, octal and prefixed integers", c_case),
        case("scanf-widths-sets", "sscanf field widths and %[ scansets, including negated sets, stop where specified", c_case),
        case("scanf-floats", "sscanf %lf and %f read exponents, signs and inf", c_case),
        case("scanf-return", "sscanf returns EOF on empty input, 0 on a mismatch, and does not count suppressed or %n conversions", c_case),
        case("fgets-lines", "fgets stops after a newline or at size-1 bytes and returns NULL at end of file", c_case),
        case("getline-grows", "getline grows the buffer for a 301-byte line, reads an unterminated last line and returns -1 at end of file", c_case),
        case("freopen-stdout", "freopen redirects stdout to a file that later output reaches", c_case),
        case("tmpfile-rw", "tmpfile returns a stream open for update that reads back what was written", c_case),
        case("error-indicator", "A failed write sets the error indicator, not end-of-file, and clearerr clears it", c_case),
        case("eof-indicator", "The end-of-file indicator is set only once a read goes past the end, and clearerr clears it", c_case),
        case("open-memstream", "open_memstream grows a buffer and updates its pointer and size on fflush", c_case),
        case("exit-flushes", "exit flushes buffered output streams before the process ends", c_case),
        case("perror-format", "perror writes the prefix, a colon and space, strerror(errno) and a newline to stderr", c_case),
        case("fclose-closes", "fclose flushes buffered output and closes the underlying descriptor", c_case),
    ]),
    category("string-stdlib", "string & stdlib", &[
        case("memcpy-memmove", "memcpy copies and memmove handles overlap in both directions, each returning the destination", c_case),
        case("memset-memchr", "memset and memchr convert their byte to unsigned char, and memchr searches past NUL only within n", c_case),
        case("memcmp-unsigned", "memcmp compares bytes as unsigned char and only within n", c_case),
        case("strlen-strnlen", "strlen and strnlen return the length, strnlen at most maxlen", c_case),
        case("strncpy-pads", "strncpy pads with NUL to n and writes no terminator for a longer source, and stpcpy returns the end", c_case),
        case("strcat-strncat", "strcat appends, and strncat appends at most n bytes and always a terminator", c_case),
        case("strcmp-unsigned", "strcmp and strncmp compare as unsigned char, and strcasecmp/strncasecmp ignore case", c_case),
        case("strchr-strrchr", "strchr and strrchr find the first and last byte, including the terminator", c_case),
        case("strstr-search", "strstr finds a needle after a partial match, returns the haystack for an empty needle and NULL when absent", c_case),
        case("strspn-strpbrk", "strspn, strcspn and strpbrk measure and find spans of accepted and rejected bytes", c_case),
        case("strtok-runs", "strtok and strtok_r skip runs of delimiters and return NULL after the last token", c_case),
        case("strdup-strndup", "strdup and strndup return new copies, strndup at most n bytes", c_case),
        case("strerror-text", "strerror gives text per error, and strerror_r into a too-small buffer returns ERANGE", c_case),
        case("strcoll-c-locale", "In the C locale strcoll orders by byte value and strxfrm preserves that order", c_case),
        case("strtol-bases", "strtol reads bases 0, 16 and 36, signs and leading space, and sets the end pointer", c_case),
        case("strtol-erange", "strtol and strtoll return LONG_MAX, LONG_MIN or LLONG_MAX with ERANGE on overflow, consuming every digit", c_case),
        case("strtol-no-digits", "strtol with no digits returns 0 with the end pointer at the start, \"0x\" without a hex digit reads 0, and base 1 fails with EINVAL", c_case),
        case("strtoul-negative", "strtoul negates \"-1\" to ULONG_MAX without error, and strtoull overflow gives ULLONG_MAX with ERANGE", c_case),
        case("strtod-forms", "strtod reads decimal, hexadecimal, infinity, nan(...) and negative zero, setting the end pointer", c_case),
        case("strtod-erange", "strtod overflow returns HUGE_VAL with ERANGE and underflow at most DBL_MIN with ERANGE", c_case),
        case("atoi-atol", "atoi, atol, atoll and atof convert leading numbers", c_case),
        case("qsort-ints", "qsort sorts 1000 integers into order without losing any, and accepts zero elements", c_case),
        case("qsort-bsearch", "qsort moves whole 24-byte records, and bsearch finds present keys and returns NULL for absent ones", c_case),
        case("malloc-alignment", "malloc returns memory aligned for max_align_t, and free(NULL) does nothing", c_case),
        case("malloc-enomem", "malloc of SIZE_MAX or SIZE_MAX/2 returns NULL with errno ENOMEM", c_case),
        case("calloc-overflow", "calloc returns zeroed memory, and NULL with ENOMEM when nmemb*size overflows", c_case),
        case("realloc-preserves", "realloc(NULL) allocates, and growing or shrinking keeps the contents", c_case),
        case("realloc-failure-keeps", "A realloc that fails returns NULL with ENOMEM and leaves the old block intact", c_case),
        case("aligned-alloc", "posix_memalign and aligned_alloc align as asked, and a non-power-of-two alignment returns EINVAL", c_case),
        case("atexit-order", "atexit handlers run in reverse order of registration when exit is called", c_case),
        case("setenv-rules", "setenv honours overwrite, unsetenv removes, and names that are empty or contain = fail with EINVAL", c_case),
        case("div-abs", "div and ldiv truncate toward zero, and abs, labs and llabs return magnitudes", c_case),
        case("rand-repeats", "rand returns values in 0..RAND_MAX and repeats its sequence after the same srand seed", c_case),
    ]),
    category("math", "math", &[
        case("sqrt-values", "sqrt is correctly rounded, keeps -0 and returns inf for inf", c_case),
        case("sqrt-domain", "sqrt(-1) returns NaN with a domain error: EDOM and FE_INVALID as math_errhandling says", c_case),
        case("errhandling", "math_errhandling is MATH_ERRNO|MATH_ERREXCEPT (Linux policy)", c_case),
        case("log-errors", "log and log10 of zero are pole errors (-HUGE_VAL, ERANGE, FE_DIVBYZERO) and log(-1) a domain error", c_case),
        case("exp-range", "exp(1000) overflows to HUGE_VAL with ERANGE and FE_OVERFLOW, and exp(-1000) underflows to +0", c_case),
        case("pow-special", "pow returns 1 for a zero exponent or base 1 even with NaN, and exact results for the IEC 60559 special cases", c_case),
        case("pow-errors", "pow of a negative base to a non-integer is a domain error, of zero to a negative power a pole error, and overflow a range error", c_case),
        case("fmod-values", "fmod takes the sign of the dividend, returns x for an infinite divisor and keeps -0", c_case),
        case("fmod-domain", "fmod of an infinite dividend or a zero divisor returns NaN with a domain error", c_case),
        case("remainder-ties", "remainder rounds the quotient to even, and remquo returns its sign and low bits", c_case),
        case("frexp-split", "frexp splits into a fraction in [0.5, 1) and an exponent, including -0 and the smallest subnormal", c_case),
        case("ldexp-scale", "ldexp scales by powers of two into the subnormal range and overflows with ERANGE and FE_OVERFLOW", c_case),
        case("modf-parts", "modf splits into fraction and integral parts with the argument's sign, including infinities", c_case),
        case("round-functions", "round, ceil, floor and trunc round as specified and keep the sign of zero results", c_case),
        case("rint-modes", "rint and nearbyint round in the current rounding direction set by fesetround", c_case),
        case("fesetround-query", "fegetround starts at FE_TONEAREST, reports what fesetround set, an invalid mode fails, and arithmetic follows the mode", c_case),
        case("lround-lrint", "lround rounds halves away from zero, lrint to even, and an out-of-range lround raises FE_INVALID", c_case),
        case("fe-exceptions", "Division by zero, invalid operations and overflow raise their flags, and feraiseexcept and feclearexcept set and clear them", c_case),
        case("feholdexcept", "feholdexcept clears and saves the flags, feupdateenv merges them back, and fegetexceptflag/fesetexceptflag restore one", c_case),
        case("fpclassify-kinds", "fpclassify and isnan, isinf, isfinite and isnormal classify NaN, infinity, subnormal, zero and normal values", c_case),
        case("nan-quiet-compare", "nan(\"\") is a NaN unequal to itself, and the quiet comparison macros return false without FE_INVALID", c_case),
        case("copysign-signbit", "copysign and fabs set and clear the sign, and signbit reads it from zeros and NaNs", c_case),
        case("hypot-no-overflow", "hypot is exact for 3,4, returns inf for inf and NaN, and does not overflow for 1e300,1e300", c_case),
        case("trig-zeros", "sin, tan and atan2 keep the signs of zeros, and atan2 and atan return the IEC 60559 multiples of pi", c_case),
        case("trig-values", "sin, cos, asin, acos and tanh are within an ulp, including sin(1e22) with full argument reduction", c_case),
        case("trig-domain", "sin and cos of infinity and acos/asin outside [-1, 1] return NaN with a domain error", c_case),
        case("exp-log-values", "exp(0) and log(1) are exact, exp2 is exact for integers, and log2, log10, log and cbrt are within an ulp", c_case),
        case("log1p-expm1", "log1p and expm1 keep -0 and tiny arguments, log1p(-1) is a pole error and log1p(-2) a domain error", c_case),
        case("float-functions", "The float functions sqrtf, floorf, fmodf, ldexpf and expf compute in float, expf overflowing with ERANGE", c_case),
        case("gamma-values", "tgamma and lgamma give known values, tgamma(+0) is a pole error and tgamma(-1) a domain error", c_case),
        case("erf-erfc", "erf and erfc give their limits at zero and infinity and erf(1) within an ulp", c_case),
        case("fma-rounding", "fma computes x*y+z with one rounding", c_case),
        case("fmax-fmin-fdim", "fmax and fmin ignore a NaN argument, and fdim returns the positive difference or +0", c_case),
        case("nextafter-steps", "nextafter steps one ulp, from zero to the smallest subnormal, and overflows from DBL_MAX with ERANGE", c_case),
        case("ilogb-logb", "ilogb, logb, scalbn and scalbln give exponents and scale, logb(0) raises FE_DIVBYZERO and ilogb(0) FE_INVALID", c_case),
    ]),
    category("locale-wchar", "locale & wide characters", &[
        case("setlocale-startup", "A program starts in the C locale, which has MB_CUR_MAX 1", c_case),
        case("setlocale-posix", "setlocale accepts POSIX and C, and selecting POSIX reports C or POSIX", c_case),
        case("setlocale-unknown", "setlocale of an unknown locale returns NULL and leaves the locale unchanged", c_case),
        case("setlocale-environment", "setlocale(LC_ALL, \"\") with no locale variables selects C, and LC_ALL overrides LC_CTYPE", c_case),
        case("localeconv-c", "localeconv in the C locale returns the values C17 7.11.2.1 gives", c_case),
        case("nl-langinfo-codeset", "nl_langinfo(CODESET) is UTF-8 in C.UTF-8 and RADIXCHAR is . in C", c_case),
        case("mbrtowc-lengths", "In C.UTF-8 mbrtowc decodes 1-, 2-, 3- and 4-byte characters and returns 0 for NUL", c_case),
        case("mbrtowc-incomplete", "mbrtowc returns (size_t)-2 for an incomplete character and completes it on the next call", c_case),
        case("mbrtowc-invalid", "mbrtowc rejects a stray byte, an overlong form and a surrogate with EILSEQ", c_case),
        case("utf8-range", "UTF-8 ends at U+10FFFF (RFC 3629, ISO/IEC 10646): mbrtowc and wcrtomb fail with EILSEQ above it and for surrogates", c_case),
        case("wcrtomb-utf8", "wcrtomb encodes U+20AC and U+10FFFF as UTF-8, and wcrtomb(NULL) returns 1", c_case),
        case("wcrtomb-c-locale", "In the C locale wcrtomb encodes ASCII and fails with EILSEQ for U+20AC", c_case),
        case("mbstowcs-round-trip", "mbstowcs and wcstombs convert a string both ways and report EILSEQ for invalid input", c_case),
        case("mbsrtowcs-source", "mbsrtowcs advances the source pointer and sets it to NULL at the terminator", c_case),
        case("mblen-mbtowc", "mblen and mbtowc report character lengths, and 0 for a stateless encoding's state query", c_case),
        case("btowc-wctob", "btowc and wctob map single-byte characters and return WEOF or EOF otherwise", c_case),
        case("wcs-functions", "wcslen, wcscmp, wcsncmp, wcscpy, wcscat, wcschr, wcsrchr, wcsstr and wcsspn work on wide strings", c_case),
        case("wmem-functions", "wmemcpy, wmemmove, wmemset, wmemchr and wmemcmp work on wide-character arrays", c_case),
        case("wcstol-wcstod", "wcstol and wcstod read wide strings, set the end pointer and report ERANGE", c_case),
        case("wctype-utf8", "In C.UTF-8 iswalpha, iswupper, iswlower, towupper and towlower handle Latin and Greek, and iswdigit only 0-9", c_case),
        case("wctype-c-locale", "In the C locale the wctype functions classify ASCII only, and towupper(WEOF) is WEOF", c_case),
        case("wctype-wctrans", "wctype, iswctype, wctrans and towctrans look up classes and mappings by name, and 0 for unknown names", c_case),
        case("fwide-orientation", "A stream starts unoriented, the first wide or byte operation orients it, and fwide cannot change that", c_case),
        case("fputws-fgetws", "fputws writes UTF-8 bytes in C.UTF-8 and fgetws reads them back", c_case),
        case("fgetwc-eilseq", "fgetwc of an invalid byte returns WEOF with EILSEQ and sets the error indicator", c_case),
        case("swprintf-wide", "swprintf formats %d %ls %lc, fails when the result does not fit, and snprintf %ls converts to UTF-8", c_case),
        case("uselocale-thread", "newlocale and uselocale give the thread C.UTF-8, and uselocale(LC_GLOBAL_LOCALE) restores the global C locale", c_case),
    ]),
    category("dirent-glob", "dirent, glob & fnmatch", &[
        case("opendir-errors", "opendir fails with ENOENT for a missing path and ENOTDIR for a file", c_case),
        case("readdir-lists", "readdir returns ., .. and every entry exactly once, then NULL without changing errno", c_case),
        case("readdir-ino-type", "readdir gives d_ino equal to stat's st_ino and d_type DT_REG and DT_DIR (Linux ABI)", c_case),
        case("readdir-empty", "readdir of an empty directory returns only . and ..", c_case),
        case("readdir-many", "readdir returns each of 300 entries exactly once", c_case),
        case("rewinddir-rescan", "rewinddir rescans and sees an entry created after opendir", c_case),
        case("telldir-seekdir", "seekdir to a telldir position makes readdir return the same entry again", c_case),
        case("dirfd-fdopendir", "dirfd returns the stream's directory descriptor, and fdopendir reads a directory descriptor and fails with ENOTDIR for a file", c_case),
        case("scandir-alphasort", "scandir applies its filter and sorts with alphasort", c_case),
        case("glob-star", "glob(\"*.c\") returns the sorted matches and a NULL-terminated gl_pathv", c_case),
        case("glob-brackets", "glob ? and [ ] bracket expressions, including [!...], match single characters", c_case),
        case("glob-nomatch", "glob with no match returns GLOB_NOMATCH, and GLOB_NOCHECK returns the pattern", c_case),
        case("glob-hidden", "glob * does not match a leading period, and a pattern starting with . does", c_case),
        case("glob-mark", "GLOB_MARK appends / to directories only", c_case),
        case("glob-append-offs", "GLOB_DOOFFS reserves NULL slots and GLOB_APPEND adds a second pattern's matches", c_case),
        case("glob-subdirs", "glob matches patterns in several path components", c_case),
        case("glob-escape", "A backslash in a glob pattern quotes the next character, unless GLOB_NOESCAPE", c_case),
        case("fnmatch-basic", "fnmatch * and ? match strings, returning 0 or FNM_NOMATCH", c_case),
        case("fnmatch-pathname", "With FNM_PATHNAME a slash is matched only by a slash, not by *, ? or a bracket expression", c_case),
        case("fnmatch-period", "With FNM_PERIOD a leading period, also after a slash with FNM_PATHNAME, is matched only explicitly", c_case),
        case("fnmatch-escape", "A backslash in fnmatch quotes the next character, unless FNM_NOESCAPE", c_case),
        case("fnmatch-brackets", "fnmatch bracket expressions: ranges, negation, ] and - as members, and character classes", c_case),
        case("fnmatch-casefold", "FNM_CASEFOLD matches without regard to case", c_case),
    ]),
    category("getopt-env", "getopt & environment", &[
        case("getopt-initial", "optind starts at 1 and opterr at 1", c_case),
        case("getopt-flags", "getopt returns each option, then -1 at the first operand with optind pointing at it", c_case),
        case("getopt-attached-arg", "getopt takes an option-argument attached to its option", c_case),
        case("getopt-separate-arg", "getopt takes an option-argument from the next argument", c_case),
        case("getopt-grouped", "getopt returns grouped options one by one and advances optind after the group", c_case),
        case("getopt-grouped-arg", "getopt takes the rest of a group as an option-argument", c_case),
        case("getopt-double-dash", "getopt returns -1 at -- and steps optind past it", c_case),
        case("getopt-stops-at-operand", "getopt returns -1 at the first operand and does not permute argv (POSIX)", c_case),
        case("getopt-lone-dash", "getopt treats - as an operand", c_case),
        case("getopt-unknown", "getopt returns ? and sets optopt for an unknown option, printing nothing when opterr is 0", c_case),
        case("getopt-diagnostic", "getopt writes a diagnostic naming an unknown option to stderr when opterr is set", c_case),
        case("getopt-missing-arg", "getopt returns ? and sets optopt when an option-argument is missing", c_case),
        case("getopt-missing-colon", "getopt returns : with no diagnostic for a missing option-argument when optstring starts with :", c_case),
        case("getopt-dash-argument", "getopt takes an option-argument that starts with -", c_case),
        case("long-basic", "getopt_long returns the val of a long option without an argument", c_case),
        case("long-required", "getopt_long takes a required argument after = or as the next argument", c_case),
        case("long-flag", "getopt_long stores val in *flag and returns 0 for an option with a flag", c_case),
        case("long-index", "getopt_long sets *longindex to the matched option's index", c_case),
        case("long-abbrev", "getopt_long accepts a unique prefix of a long option", c_case),
        case("long-ambiguous", "getopt_long returns ? for an ambiguous prefix", c_case),
        case("long-optional", "getopt_long gives an optional argument only after =, else optarg NULL", c_case),
        case("long-unknown", "getopt_long returns ? for an unknown long option", c_case),
        case("long-missing", "getopt_long returns : for a missing required argument when optstring starts with :", c_case),
        case("long-mixed-short", "getopt_long handles short and long options together", c_case),
        case("getsubopt-tokens", "getsubopt returns token indexes and values, and -1 with the whole token for an unknown one", c_case),
        case("environ-startup", "environ at startup is main's envp and holds exactly the environment execve passed", c_case),
        case("getenv-lookup", "getenv matches whole names only and returns NULL for missing ones", c_case),
        case("putenv-shares", "putenv makes the caller's string part of the environment, so changing it changes getenv's result", c_case),
        case("putenv-replaces", "putenv of an existing name replaces it, leaving one entry", c_case),
        case("setenv-environ", "setenv and unsetenv update environ, unsetenv of a missing name succeeds, and empty or = names fail with EINVAL", c_case),
        case("env-fork-inherits", "A child created by fork sees variables set with setenv", c_case),
    ]),
    category("pthread", "pthreads through libc", &[
        case("create-join", "pthread_join returns the value the thread's start routine returned", c_case),
        case("join-self", "Linux policy: pthread_join of the calling thread fails with EDEADLK", c_case),
        case("detach-runs", "pthread_detach succeeds and the detached thread runs", c_case),
        case("attr-detached", "Threads are joinable by default, an invalid detach state is EINVAL, and a thread created detached runs", c_case),
        case("exit-value", "pthread_join returns the value given to pthread_exit from a nested call", c_case),
        case("self-equal", "pthread_self in a thread equals the ID pthread_create returned, and differs from its creator's", c_case),
        case("many-threads", "Sixteen threads created at once all run and are joined", c_case),
        case("mutex-trylock", "pthread_mutex_trylock of a locked mutex returns EBUSY, even for the owner, and succeeds once unlocked", c_case),
        case("mutex-errorcheck", "An error-checking mutex reports EDEADLK on relock and EPERM on unlock by a non-owner or when unlocked", c_case),
        case("mutex-recursive", "A recursive mutex is released after as many unlocks as locks, then reports EPERM", c_case),
        case("mutex-contention", "Four threads adding 20000 each under a mutex lose no updates", c_case),
        case("mutex-timedlock", "pthread_mutex_timedlock of a held mutex returns ETIMEDOUT, not before its deadline", c_case),
        case("mutexattr-type", "The default mutex type is PTHREAD_MUTEX_DEFAULT, settype reports EINVAL for an invalid type, and gettype reports what settype set", c_case),
        case("mutex-robust", "A robust mutex whose owner exited returns EOWNERDEAD, and pthread_mutex_consistent recovers it", c_case),
        case("cond-signal", "pthread_cond_signal wakes a waiter that sees the predicate set", c_case),
        case("cond-broadcast", "pthread_cond_broadcast wakes all four waiters", c_case),
        case("cond-timedwait-timeout", "pthread_cond_timedwait returns ETIMEDOUT not before its deadline, with the mutex held again", c_case),
        case("cond-monotonic", "A condition variable set to CLOCK_MONOTONIC times its wait on that clock; the default clock is CLOCK_REALTIME", c_case),
        case("cond-past-deadline", "pthread_cond_timedwait with a deadline already past returns ETIMEDOUT at once", c_case),
        case("rwlock-readers", "A read-locked rwlock admits another reader and refuses a writer with EBUSY", c_case),
        case("rwlock-writer", "A write-locked rwlock refuses readers and writers with EBUSY, and a timed read lock times out", c_case),
        case("rwlock-self-deadlock", "Linux policy: relocking an rwlock the caller holds for writing fails with EDEADLK", c_case),
        case("once-only", "pthread_once runs its routine once and returns in every thread only after it finished", c_case),
        case("keys-destructor", "Thread-specific data is per thread and its destructor runs at thread exit with each thread's value", c_case),
        case("cancel-testcancel", "A cancelled thread ends at pthread_testcancel and joins as PTHREAD_CANCELED", c_case),
        case("cancel-blocking-read", "read is a cancellation point: a thread blocked in read is cancelled", c_case),
        case("cancel-disabled", "A cancel sent while cancellation is disabled stays pending and acts once enabled", c_case),
        case("cancel-cleanup", "Cleanup handlers pushed with pthread_cleanup_push run when the thread is cancelled", c_case),
        case("cancel-completed-read", "A cancel that arrives as read completes keeps the byte read: the cancelled thread's count and the pipe's remainder add up", c_case),
        case("cancel-disabled-no-eintr", "A cancel sent while cancellation is disabled does not make nanosleep or a blocking read fail with EINTR", c_case),
        case("cancel-stdio-unlocks", "A thread cancelled while blocked in stdio output leaves the stream unlocked, and fclose and fopen still work", c_case),
        case("cancel-cond-signal", "A waiter cancelled in pthread_cond_wait does not consume a concurrent pthread_cond_signal another thread waits for", c_case),
        case("cancel-pending-exec", "A program exec'd by a thread with cancellation disabled and a cancel request pending starts and runs", c_case),
        case("barrier-wait", "pthread_barrier_wait releases all four threads together and gives exactly one PTHREAD_BARRIER_SERIAL_THREAD", c_case),
        case("spin-lock", "pthread_spin_trylock of a held spin lock returns EBUSY and succeeds once unlocked", c_case),
        case("attr-stacksize", "pthread_attr_setstacksize refuses less than PTHREAD_STACK_MIN and a thread runs on the size set", c_case),
        case("errno-per-thread", "errno is per thread", c_case),
        case("sigmask-per-thread", "pthread_sigmask changes only the calling thread's signal mask", c_case),
        case("sched-affinity", "sched_setaffinity narrows and restores the CPU_* set sched_getaffinity reports, with ESRCH for a negative pid and EINVAL for a short or empty set", c_case),
    ]),
    category("busybox", "BusyBox built on our libc", &[
        case("sh-arith", "sh expands $((6*7)) to 42", sh_arith),
        case("sh-pipeline", "sh runs printf piped into sort", sh_pipeline),
        case("sh-status", "sh reports $? of false as 1 and exits with the status given to exit", sh_status),
        case("sh-control", "sh runs a for loop with an if and test inside", sh_control),
        case("sh-redirect", "sh redirects with >, >> and <", sh_redirect),
        case("echo", "echo prints its arguments separated by spaces with a newline", echo),
        case("cat", "cat concatenates files and - for standard input", cat),
        case("ls", "ls lists a directory's names sorted, one per line when not on a terminal", ls),
        case("ls-long", "ls -l shows a file's mode, size and name", ls_long),
        case("cp", "cp copies a file's contents", cp),
        case("mv", "mv renames a file", mv),
        case("rm", "rm removes a file and exits 1 for a missing one", rm),
        case("mkdir", "mkdir -p creates a nested path", mkdir),
        case("sed", "sed s/// substitutes and sed -n 2p prints one line", sed),
        case("awk-sum", "awk sums a field across lines in an END block", awk_sum),
        case("awk-printf", "awk printf formats %05.1f, %-3s and %x", awk_printf),
        case("grep-match", "grep prints the matching lines and exits 0", grep_match),
        case("grep-status", "grep -E -c counts matches, and grep exits 1 when nothing matches", grep_status),
        case("sort", "sort -n orders numerically and sort -r -u reverses and drops duplicates", sort),
        case("uniq", "uniq -c counts adjacent repeated lines", uniq),
        case("wc", "wc -l -w -c counts lines, words and bytes", wc),
        case("head", "head -n 2 prints the first two lines", head),
        case("tail", "tail -n 1 prints the last line", tail),
        case("cut", "cut -d: -f2 selects a field", cut),
        case("tr", "tr a-z A-Z translates a range", tr),
        case("find", "find . -name '*.txt' finds files in subdirectories", find),
        case("tar", "tar creates an archive, extracts it and lists it", tar),
        case("xargs", "xargs passes standard input as arguments to echo", xargs),
        case("printf", "printf formats %s, %d and %5.2f", printf),
        case("expr", "expr evaluates 6 * 7", expr),
        case("test", "test 1 -lt 2 exits 0 and test 2 -lt 1 exits 1", test),
        case("md5sum", "md5sum of \"abc\" on standard input is 900150983cd24fb0d6963f7d28e17f72", md5sum),
        case("basename", "basename strips the directory and a suffix", basename),
        case("dirname", "dirname strips the last component", dirname),
        case("date", "date -u -d @0 formats the epoch as 1970-01-01 00:00:00", date),
    ]),
]);

fn now_ms() -> u64 {
    time::now_monotonic().map(|t| (t.as_nanos() / 1_000_000) as u64).unwrap_or(0)
}

/// A directory of the case's own under /tmp, removed with everything in it at the end.
struct Scratch {
    dir: String,
}

impl Scratch {
    fn new() -> Result<Scratch, CaseError> {
        let pid = process::getpid()?.raw();
        let dir = format!("/tmp/libc-{pid}");
        remove_tree(std::path::Path::new(&dir));
        fs::mkdir(&dir, 0o755).map_err(|e| format!("could not create the scratch directory {dir}: {e}"))?;
        Ok(Scratch { dir })
    }

    fn path(&self, name: &str) -> String {
        format!("{}/{name}", self.dir)
    }

    fn write(&self, name: &str, text: &str) -> CaseResult {
        let path = self.path(name);
        if let Some(parent) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("could not write {path}: {e}").into())
    }

    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.path(name)).ok()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        remove_tree(std::path::Path::new(&self.dir));
    }
}

/// Remove `path` and, for a directory, everything in it, using only read_dir, unlink
/// and rmdir.
fn remove_tree(path: &std::path::Path) {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    remove_tree(&entry.path());
                }
            }
            let _ = std::fs::remove_dir(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// What a program the suite started left behind.
struct Ran {
    status: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    result: Vec<u8>,
}

impl Ran {
    fn ended(&self) -> String {
        if process::wifexited(self.status) {
            format!("exit status {}", process::wexitstatus(self.status))
        } else if process::wifsignaled(self.status) {
            format!("signal {}", process::wtermsig(self.status))
        } else {
            format!("wait status {:#x}", self.status)
        }
    }

    fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn err_text(&self) -> String {
        let text = String::from_utf8_lossy(&self.stderr);
        let text = text.trim();
        if text.is_empty() { String::from("nothing on stderr") } else { format!("stderr {:?}", text) }
    }
}

fn cstrings(items: &[String]) -> (Vec<CString>, Vec<*const u8>) {
    let owned: Vec<CString> = items.iter().map(|s| CString::new(s.as_str()).unwrap_or_default()).collect();
    let mut ptrs: Vec<*const u8> = owned.iter().map(|c| c.as_ptr() as *const u8).collect();
    ptrs.push(core::ptr::null());
    (owned, ptrs)
}

/// Close-on-exec pipe ends.
fn pipe() -> Result<(Fd, Fd), CaseError> {
    io::pipe2(status_flags::O_CLOEXEC).map_err(|e| format!("pipe failed: {e}").into())
}

fn nonblocking(fd: Fd) {
    if let Ok(flags) = io::fcntl(fd, fcntl_cmd::F_GETFL, 0) {
        let _ = io::fcntl(fd, fcntl_cmd::F_SETFL, flags | status_flags::O_NONBLOCK as i64);
    }
}

/// Run `path` with `args` (an argument equal to RESULT_FD names the result pipe) in
/// `dir`, feeding it `input` and collecting its stdout, stderr and result pipe until it
/// exits, or killing it 1.5 seconds before the case's limit.
fn run(path: &str, args: &[&str], dir: &str, input: &[u8]) -> Result<Ran, CaseError> {
    let (in_r, in_w) = pipe()?;
    let (out_r, out_w) = pipe()?;
    let (err_r, err_w) = pipe()?;
    let (res_r, res_w) = pipe()?;
    let (exec_r, exec_w) = pipe()?;
    let argv: Vec<String> = args.iter()
        .map(|a| if *a == RESULT_FD { res_w.raw().to_string() } else { a.to_string() })
        .collect();
    let env: Vec<String> = ENV.iter().map(|e| e.to_string()).collect();
    let (_argv_owned, argv_ptrs) = cstrings(&argv);
    let (_env_owned, env_ptrs) = cstrings(&env);
    let cpath = CString::new(path).map_err(|_| "program path holds a NUL")?;
    let cdir = CString::new(dir).map_err(|_| "directory path holds a NUL")?;

    let child = match process::fork() {
        Ok(process::ForkResult::Child) => {
            let why = (|| -> Result<(), String> {
                process::chdir(cdir.as_bytes_with_nul()).map_err(|e| format!("chdir {dir} failed: {e}"))?;
                io::dup2(in_r, Fd::STDIN).map_err(|e| format!("dup2 failed: {e}"))?;
                io::dup2(out_w, Fd::STDOUT).map_err(|e| format!("dup2 failed: {e}"))?;
                io::dup2(err_w, Fd::STDERR).map_err(|e| format!("dup2 failed: {e}"))?;
                io::fcntl(res_w, fcntl_cmd::F_SETFD, 0).map_err(|e| format!("fcntl failed: {e}"))?;
                match process::execve(cpath.as_bytes_with_nul(), argv_ptrs.as_ptr(), env_ptrs.as_ptr()) {
                    Ok(never) => match never {},
                    Err(e) => Err(format!("execve {path} failed: {e}")),
                }
            })();
            if let Err(why) = why {
                let _ = io::write(exec_w, why.as_bytes());
            }
            process::exit(127);
        }
        Ok(process::ForkResult::Parent(pid)) => pid.raw() as i32,
        Err(e) => {
            for fd in [in_r, in_w, out_r, out_w, err_r, err_w, res_r, res_w, exec_r, exec_w] {
                let _ = io::close(fd);
            }
            return Err(format!("fork failed: {e}").into());
        }
    };
    for fd in [in_r, out_w, err_w, res_w, exec_w] {
        let _ = io::close(fd);
    }
    for fd in [in_w, out_r, err_r, res_r, exec_r] {
        nonblocking(fd);
    }

    let allowed = case_ms_left().saturating_sub(RESERVE_MS);
    let deadline = now_ms() + allowed;
    let mut readers = [(out_r, Vec::new()), (err_r, Vec::new()), (res_r, Vec::new()), (exec_r, Vec::new())];
    let mut open = [true; 4];
    let mut writer = if input.is_empty() { let _ = io::close(in_w); None } else { Some(in_w) };
    let mut sent = 0;
    let mut status = None;
    let mut exited_at = 0;
    loop {
        let mut fds: Vec<PollFd> = Vec::new();
        for (i, (fd, _)) in readers.iter().enumerate() {
            if open[i] {
                fds.push(PollFd::new(*fd, poll_events::POLLIN));
            }
        }
        if let Some(w) = writer {
            fds.push(PollFd::new(w, poll_events::POLLOUT));
        }
        if !fds.is_empty() {
            let _ = io::poll(&mut fds, 10);
        }
        for (i, (fd, buf)) in readers.iter_mut().enumerate() {
            if !open[i] {
                continue;
            }
            let mut chunk = [0u8; 4096];
            loop {
                match io::read(*fd, &mut chunk) {
                    Ok(0) => {
                        open[i] = false;
                        let _ = io::close(*fd);
                        break;
                    }
                    Ok(n) => {
                        if buf.len() < 1 << 20 {
                            buf.extend_from_slice(&chunk[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        if let Some(w) = writer {
            match io::write(w, &input[sent..]) {
                Ok(n) => sent += n,
                Err(Error::Os(Errno::EAGAIN)) => {}
                Err(_) => sent = input.len(),
            }
            if sent >= input.len() {
                let _ = io::close(w);
                writer = None;
            }
        }
        if status.is_none() {
            let mut s = 0;
            if let Ok(pid) = process::waitpid(child, &mut s, process::WNOHANG) {
                if pid.raw() as i32 == child {
                    status = Some(s);
                    exited_at = now_ms();
                }
            }
        }
        if let Some(s) = status {
            // Done once every pipe is closed; a descendant still holding one gets 200 ms.
            if !open.iter().any(|o| *o) || now_ms() > exited_at + 200 {
                for (i, (fd, _)) in readers.iter().enumerate() {
                    if open[i] {
                        let _ = io::close(*fd);
                    }
                }
                if let Some(w) = writer {
                    let _ = io::close(w);
                }
                let [(_, stdout), (_, stderr), (_, result), (_, exec_err)] = readers;
                if !exec_err.is_empty() {
                    return Err(String::from_utf8_lossy(&exec_err).into_owned().into());
                }
                return Ok(Ran { status: s, stdout, stderr, result });
            }
        } else if now_ms() > deadline {
            let _ = signal::kill(child, signal::SIGKILL);
            let mut s = 0;
            let _ = process::waitpid(child, &mut s, 0);
            for (i, (fd, _)) in readers.iter().enumerate() {
                if open[i] {
                    let _ = io::close(*fd);
                }
            }
            if let Some(w) = writer {
                let _ = io::close(w);
            }
            return Err(format!("{path} did not finish within {allowed} ms and was killed").into());
        }
    }
}

/// Run the running case's C function in its category's program.
fn c_case() -> CaseResult {
    let name = running_case().ok_or("the case could not tell which case it is")?;
    let (category, id) = name.split_once('/').ok_or("the case name has no category")?;
    let program = format!("libc-{category}_test");
    let path = format!("{CASE_DIR}/{program}");
    if fs::access(&path, fs::X_OK).is_err() {
        return fail(format!("{path} is not installed: the C cases are not yet built against Breenix's own C library"));
    }
    let scratch = Scratch::new()?;
    let ran = run(&path, &[&program, id, RESULT_FD], &scratch.dir, b"")?;
    let report = String::from_utf8_lossy(&ran.result).into_owned();
    let line = report.lines().next().unwrap_or("");
    if line == "PASS" {
        if process::wifexited(ran.status) && process::wexitstatus(ran.status) == 0 {
            return Ok(());
        }
        return fail(format!("the C case reported PASS but ended with {}", ran.ended()));
    }
    if let Some(why) = line.strip_prefix("FAIL ") {
        return fail(why);
    }
    if line.is_empty() {
        return fail(format!("the C case ended with {} without reporting a result", ran.ended()));
    }
    fail(format!("the C case reported {line:?}, which is not PASS or FAIL"))
}

/// Run a BusyBox applet in `scratch` with `args` and `input` on stdin.
fn busybox(scratch: &Scratch, args: &[&str], input: &str) -> Result<Ran, CaseError> {
    if fs::access(BUSYBOX, fs::X_OK).is_err() {
        return Err(format!("{BUSYBOX} is not installed: BusyBox is not yet built against Breenix's own C library").into());
    }
    let mut argv = vec!["busybox"];
    argv.extend_from_slice(args);
    run(BUSYBOX, &argv, &scratch.dir, input.as_bytes())
}

/// Pass when the applet exited with `status` and printed exactly `stdout`.
fn expect(what: &str, ran: &Ran, stdout: &str, status: i32) -> CaseResult {
    let exited = process::wifexited(ran.status) && process::wexitstatus(ran.status) == status;
    if !exited {
        return fail(format!("{what} ended with {}, expected exit status {status} ({})", ran.ended(), ran.err_text()));
    }
    if ran.out() != stdout {
        return fail(format!("{what} printed {:?}, expected {stdout:?}", ran.out()));
    }
    Ok(())
}

fn bb(args: &[&str], input: &str, stdout: &str, status: i32) -> CaseResult {
    let scratch = Scratch::new()?;
    let ran = busybox(&scratch, args, input)?;
    expect(&args.join(" "), &ran, stdout, status)
}

fn sh_arith() -> CaseResult { bb(&["sh", "-c", "echo $((6*7))"], "", "42\n", 0) }

fn sh_pipeline() -> CaseResult { bb(&["sh", "-c", "printf 'b\\na\\n' | sort"], "", "a\nb\n", 0) }

fn sh_status() -> CaseResult { bb(&["sh", "-c", "false; echo $?; exit 3"], "", "1\n", 3) }

fn sh_control() -> CaseResult {
    bb(&["sh", "-c", "for i in 1 2 3; do if [ $i -ne 2 ]; then echo $i; fi; done"], "", "1\n3\n", 0)
}

fn sh_redirect() -> CaseResult { bb(&["sh", "-c", "echo x > f; echo y >> f; cat < f"], "", "x\ny\n", 0) }

fn echo() -> CaseResult { bb(&["echo", "a", "b  c"], "", "a b  c\n", 0) }

fn cat() -> CaseResult {
    let scratch = Scratch::new()?;
    scratch.write("a", "1\n")?;
    scratch.write("b", "2\n")?;
    let ran = busybox(&scratch, &["cat", "a", "-", "b"], "s\n")?;
    expect("cat a - b", &ran, "1\ns\n2\n", 0)
}

fn ls() -> CaseResult {
    let scratch = Scratch::new()?;
    for name in ["b", "a", "c"] {
        scratch.write(name, "")?;
    }
    let ran = busybox(&scratch, &["ls"], "")?;
    expect("ls", &ran, "a\nb\nc\n", 0)
}

fn ls_long() -> CaseResult {
    let scratch = Scratch::new()?;
    scratch.write("f", "hello")?;
    let ran = busybox(&scratch, &["ls", "-l", "f"], "")?;
    expect("ls -l f", &ran, &ran.out(), 0)?;
    let out = ran.out();
    let fields: Vec<&str> = out.split_whitespace().collect();
    if !out.starts_with("-rw") || fields.get(4) != Some(&"5") || fields.last() != Some(&"f") {
        return fail(format!("ls -l f printed {out:?}, expected a line starting -rw with size 5 and name f"));
    }
    Ok(())
}

fn cp() -> CaseResult {
    let scratch = Scratch::new()?;
    scratch.write("a", "data\n")?;
    let ran = busybox(&scratch, &["cp", "a", "b"], "")?;
    expect("cp a b", &ran, "", 0)?;
    match scratch.read("b") {
        Some(text) if text == "data\n" => Ok(()),
        other => fail(format!("after cp a b, b holds {other:?}, expected \"data\\n\"")),
    }
}

fn mv() -> CaseResult {
    let scratch = Scratch::new()?;
    scratch.write("a", "data\n")?;
    let ran = busybox(&scratch, &["mv", "a", "b"], "")?;
    expect("mv a b", &ran, "", 0)?;
    if scratch.read("a").is_some() {
        return fail("after mv a b, a still exists");
    }
    match scratch.read("b") {
        Some(text) if text == "data\n" => Ok(()),
        other => fail(format!("after mv a b, b holds {other:?}, expected \"data\\n\"")),
    }
}

fn rm() -> CaseResult {
    let scratch = Scratch::new()?;
    scratch.write("a", "x")?;
    let ran = busybox(&scratch, &["rm", "a"], "")?;
    expect("rm a", &ran, "", 0)?;
    if scratch.read("a").is_some() {
        return fail("after rm a, a still exists");
    }
    let ran = busybox(&scratch, &["rm", "missing"], "")?;
    expect("rm missing", &ran, "", 1)
}

fn mkdir() -> CaseResult {
    let scratch = Scratch::new()?;
    let ran = busybox(&scratch, &["mkdir", "-p", "x/y/z"], "")?;
    expect("mkdir -p x/y/z", &ran, "", 0)?;
    match std::fs::metadata(scratch.path("x/y/z")) {
        Ok(meta) if meta.is_dir() => Ok(()),
        _ => fail("after mkdir -p x/y/z, x/y/z is not a directory"),
    }
}

fn sed() -> CaseResult {
    bb(&["sed", "s/world/there/"], "hello world\n", "hello there\n", 0)?;
    bb(&["sed", "-n", "2p"], "1\n2\n3\n", "2\n", 0)
}

fn awk_sum() -> CaseResult { bb(&["awk", "{s+=$2} END {print s}"], "a 1\nb 2\n", "3\n", 0) }

fn awk_printf() -> CaseResult {
    bb(&["awk", "BEGIN{printf \"%05.1f|%-3s|%x\\n\", 3.14159, \"a\", 255}"], "", "003.1|a  |ff\n", 0)
}

fn grep_match() -> CaseResult { bb(&["grep", "foo"], "foo\nbar\nfood\n", "foo\nfood\n", 0) }

fn grep_status() -> CaseResult {
    bb(&["grep", "-E", "-c", "a+b"], "ab\naab\nb\n", "2\n", 0)?;
    bb(&["grep", "zzz"], "foo\n", "", 1)
}

fn sort() -> CaseResult {
    bb(&["sort", "-n"], "10\n9\n100\n", "9\n10\n100\n", 0)?;
    bb(&["sort", "-r", "-u"], "b\na\nb\n", "b\na\n", 0)
}

fn uniq() -> CaseResult {
    let scratch = Scratch::new()?;
    let ran = busybox(&scratch, &["uniq", "-c"], "a\na\nb\n")?;
    expect("uniq -c", &ran, &ran.out(), 0)?;
    let lines: Vec<String> = ran.out().lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    if lines != ["2 a", "1 b"] {
        return fail(format!("uniq -c printed {:?}, expected counts 2 a and 1 b", ran.out()));
    }
    Ok(())
}

fn wc() -> CaseResult {
    let scratch = Scratch::new()?;
    let ran = busybox(&scratch, &["wc", "-l", "-w", "-c"], "one two\nthree\n")?;
    expect("wc -l -w -c", &ran, &ran.out(), 0)?;
    let fields: Vec<String> = ran.out().split_whitespace().map(String::from).collect();
    if fields != ["2", "3", "14"] {
        return fail(format!("wc -l -w -c printed {:?}, expected 2 lines, 3 words and 14 bytes", ran.out()));
    }
    Ok(())
}

fn head() -> CaseResult { bb(&["head", "-n", "2"], "1\n2\n3\n4\n", "1\n2\n", 0) }

fn tail() -> CaseResult { bb(&["tail", "-n", "1"], "1\n2\n3\n", "3\n", 0) }

fn cut() -> CaseResult { bb(&["cut", "-d:", "-f2"], "a:b:c\n", "b\n", 0) }

fn tr() -> CaseResult { bb(&["tr", "a-z", "A-Z"], "Hello\n", "HELLO\n", 0) }

fn find() -> CaseResult {
    let scratch = Scratch::new()?;
    scratch.write("x.txt", "")?;
    scratch.write("d/y.txt", "")?;
    scratch.write("z.c", "")?;
    let ran = busybox(&scratch, &["find", ".", "-name", "*.txt"], "")?;
    expect("find . -name *.txt", &ran, &ran.out(), 0)?;
    let mut lines: Vec<&str> = Vec::new();
    let out = ran.out();
    lines.extend(out.lines());
    lines.sort_unstable();
    if lines != ["./d/y.txt", "./x.txt"] {
        return fail(format!("find . -name *.txt printed {out:?}, expected ./x.txt and ./d/y.txt"));
    }
    Ok(())
}

fn tar() -> CaseResult {
    let scratch = Scratch::new()?;
    scratch.write("f", "tar me\n")?;
    let ran = busybox(&scratch, &["sh", "-c", "tar cf a.tar f && rm f && tar xf a.tar && cat f && tar tf a.tar"], "")?;
    expect("tar cf, xf and tf", &ran, "tar me\nf\n", 0)
}

fn xargs() -> CaseResult { bb(&["xargs", "echo"], "a\nb\n", "a b\n", 0) }

fn printf() -> CaseResult { bb(&["printf", "%s-%d-%5.2f\\n", "x", "5", "3.14159"], "", "x-5- 3.14\n", 0) }

fn expr() -> CaseResult { bb(&["expr", "6", "*", "7"], "", "42\n", 0) }

fn test() -> CaseResult {
    bb(&["test", "1", "-lt", "2"], "", "", 0)?;
    bb(&["test", "2", "-lt", "1"], "", "", 1)
}

fn md5sum() -> CaseResult { bb(&["md5sum"], "abc", "900150983cd24fb0d6963f7d28e17f72  -\n", 0) }

fn basename() -> CaseResult { bb(&["basename", "/a/b/c.txt", ".txt"], "", "c\n", 0) }

fn dirname() -> CaseResult { bb(&["dirname", "/a/b/c.txt"], "", "/a/b\n", 0) }

fn date() -> CaseResult { bb(&["date", "-u", "-d", "@0", "+%Y-%m-%d %H:%M:%S"], "", "1970-01-01 00:00:00\n", 0) }

fn main() { SUITE.run() }
