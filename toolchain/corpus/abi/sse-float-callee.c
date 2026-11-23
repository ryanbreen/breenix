/* AMD64 psABI §3.2.3: the ninth SSE argument goes on the stack. */
double abi_sse(float a, double b, float c, double d, float e, double f,
               float g, double h, float i, int tag)
{
    return a + b + c + d + e + f + g + h + i + tag;
}

float abi_sse_return(double value)
{
    return (float)(value * 0.5);
}
