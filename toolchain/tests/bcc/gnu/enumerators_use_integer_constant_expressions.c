static const int values[] = {1, 2, 3};
enum {
    COUNT = sizeof(values) / sizeof(values[0]),
    FLAG = 1 << COUNT,
    NEG = 1 - FLAG,
    NEXT
};

int main(void) {
    return COUNT == 3 && FLAG == 8 && NEG == -7 && NEXT == -6 ? 42 : 1;
}
