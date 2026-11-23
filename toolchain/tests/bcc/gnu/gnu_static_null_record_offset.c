struct value {
    char tag;
    long payload;
};
static const int offset = (int)(long)&((struct value *)0)->payload;
int main(void) {
    static const int offsets[] = { (int)(long)&((struct value *)0)->payload };
    return offset == 8 && offsets[0] == 8 ? 42 : 1;
}
