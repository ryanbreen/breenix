/* AAPCS64 2025Q1 §§5.5, 6.8.2 and 6.9. */
struct Quads {
    long double first;
    long double second;
};

extern long double bias;
struct Quads turn(struct Quads value);
long double many(long double a, long double b, long double c, long double d,
                 long double e, long double f, long double g, long double h,
                 long double i, struct Quads tail);
int variadic(long a, long b, long c, long d, long e, long f, long g, long h,
             long double named, ...);
int conversions(int i, long l, unsigned u, unsigned long ul, float f, double d);
