/* AAPCS64 2025Q1 §§5.10.5 and 6.8.2 C.1–C.6. */
struct Floats { float first, second; };
struct Doubles { double lanes[3]; };

typedef float Float2 __attribute__((vector_size(8)));
typedef double Double2 __attribute__((vector_size(16)));
union FloatBits { Float2 vector; float lanes[2]; };
union DoubleBits { Double2 vector; double lanes[2]; };
struct Vectors { Double2 first, second; };

struct Floats floats(float, struct Floats, double, struct Floats (*)(struct Floats));
struct Doubles doubles(float, double, float, double, float, double, float,
                       struct Doubles, double, long);
Float2 vector(Float2);
struct Vectors vectors(float, struct Vectors);
