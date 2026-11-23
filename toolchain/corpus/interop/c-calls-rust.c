/* AMD64 psABI §3.2.3: scalars, mixed eightbytes and a C ABI callback. */
struct Mixed { int i; double d; };
double rust_interop_scalar(int, double);
struct Mixed rust_interop(struct Mixed, struct Mixed (*)(struct Mixed));

static struct Mixed callback(struct Mixed value)
{
    if (value.i != 17 || value.d != 2.5) {
        value.i = -1;
        return value;
    }
    value.i = 23;
    value.d = 4.5;
    return value;
}

int main(void)
{
    struct Mixed value;

    if (rust_interop_scalar(5, 1.25) != 6.25)
        return 1;
    value.i = 11;
    value.d = 1.5;
    value = rust_interop(value, callback);
    return value.i != 29 || value.d != 8.5;
}
