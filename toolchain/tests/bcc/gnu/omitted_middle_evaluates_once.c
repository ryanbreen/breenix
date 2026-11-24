int main(void) {
    int i = 40;
    int j = 0;
    int a = i++ ?: ++j;
    int b = j++ ?: ++j;
    return i == 41 && j == 2 ? a + b : 1;
}
