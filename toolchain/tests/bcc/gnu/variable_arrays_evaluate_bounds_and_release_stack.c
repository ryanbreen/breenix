int main(void) {
    int bound = 3;
    int sum = 0;
    for (int i = 0; i < 3; i++) {
        int values[bound++];
        if (sizeof(values) != (unsigned long)(bound - 1) * sizeof(int))
            return 1;
        values[bound - 2] = 20 + i;
        if (i == 0)
            continue;
        sum += values[bound - 2];
        if (i == 1) {
            int second[bound];
            second[bound - 1] = 21;
            sum += second[bound - 1];
            goto finish;
        }
    }
finish:
    return bound == 5 ? sum : 2;
}
