#include <stdarg.h>

struct Pair { long first, second; };
struct Large { long first, second, third; };
struct Doubles { double first, second; };

int take_int(va_list *);
int inspect(long, double, ...);
int after_named_stack(long, long, long, long, long, long, long, long, long, ...);
