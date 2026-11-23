/* AAPCS64 2025Q1 §§5.10.5, 6.8.2 and 6.9. */
struct Floats { float lanes[3]; };
struct Pair { long first, second; };
struct Large { long first, second, third; };
/* A 16-byte-aligned non-HFA, without doing binary128 arithmetic. */
union Aligned { long double alignment; long words[2]; };

union Aligned aligned(long, union Aligned, long);
struct Large exhausted(long, long, long, long, long, long, long,
                       struct Pair, long, struct Large, struct Floats);
