static int value(int n) {
    switch (n) {
    case -4 ... -2: return 9;
    case 10 ... 12: return 4;
    case 13: return 3;
    default: return 1;
    }
}
int main(void) {
    return value(-5) + value(-4) + value(-3) + value(-2) + value(-1)
         + value(10) + value(11) + value(12) + value(13) + value(14) + 1;
}
