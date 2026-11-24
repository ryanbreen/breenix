struct trailing { char tag; long data[0]; };
static long empty[0];
int main(void) {
    struct trailing x;
    long local[0];
    return sizeof(x) + sizeof(x.data) + sizeof(empty) + sizeof(local)
         + __builtin_offsetof(struct trailing, data) + _Alignof(long[0]) + 18;
}
