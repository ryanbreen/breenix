int main(void) {
    int n = 3;
    int result = ({ int n = 6; int sum = 0;
                    for (int i = 0; i < n; ++i) sum += i;
                    sum; });
    ({ n += 3; if (n == 0) n = 99; });
    return 2 * result + 2 * n;
}
