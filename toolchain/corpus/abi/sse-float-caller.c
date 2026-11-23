/* AMD64 psABI §3.2.3: separate INTEGER and SSE register allocation. */
double abi_sse(float, double, float, double, float, double, float, double,
               float, int);
float abi_sse_return(double);

int main(void)
{
    double sum = abi_sse(0.5f, 1.0, 2.0f, 3.0, 4.0f, 5.0, 6.0f, 7.0, 8.5f, 9);
    return sum != 46.0 || abi_sse_return(sum) != 23.0f;
}
