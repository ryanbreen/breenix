/* N1570 6.3.1.8, signed and unsigned operands of the same rank. */
int main(void)
{
    int negative = -3;
    unsigned value = 5;
    unsigned sum = negative + value;
    return sum == 2U ? 0 : 1;
}
