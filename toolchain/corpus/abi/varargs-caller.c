/* N1570 §6.5.2.2: default argument promotions. AMD64 psABI §3.5.7. */
int abi_variadic(int, ...);

int main(void)
{
    short integer = 1;
    float fraction = 2.5f;
    return abi_variadic(10, integer, 2, 3, 4, 5, 6, fraction, 7) != 43;
}
