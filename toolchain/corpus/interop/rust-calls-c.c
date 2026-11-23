/* AMD64 psABI §3.2.3: Rust supplies the C arguments and callback. */
struct Mixed { int i; double d; };
int rust_call_c(void);

struct Mixed c_interop(int tag, double scale, struct Mixed value,
                       struct Mixed (*callback)(struct Mixed))
{
    if (tag != 7 || scale != 1.5 || value.i != 13 || value.d != 2.25) {
        value.i = -1;
        return value;
    }
    value.i += tag;
    value.d += scale;
    return callback(value);
}

int main(void)
{
    return rust_call_c();
}
