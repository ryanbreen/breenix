/* AAPCS64 2025Q1 §§6.8–6.9 and N1570 §§6.3.1, 6.5.2.2. */
int probe(float first, int tag, ...);
double quiet_nan(void);

static float half(float value)
{
    return value * 0.5f;
}

double receive(float f0, int i0, int i1, int i2, int i3, int i4,
               int i5, int i6, int i7, int i8, double f1, double f2,
               double f3, double f4, double f5, double f6, double f7,
               float f8, int i9, double f9)
{
    if (i0 != 11 || i1 != 12 || i2 != 13 || i3 != 14 || i4 != 15 ||
        i5 != 16 || i6 != 17 || i7 != 18 || i8 != 19 || i9 != 20)
        return -1.0;
    if (f0 != 1.5f || f1 != 2.5 || f2 != 3.5 || f3 != 4.5 ||
        f4 != 5.5 || f5 != 6.5 || f6 != 7.5 || f7 != 8.5 ||
        f8 != 9.5f || f9 != 10.5)
        return -2.0;

    double nan = quiet_nan();
    if (nan == nan || nan < f9 || nan <= f9 || nan > f9 || nan >= f9 ||
        !(nan != nan) || !nan || (_Bool)-0.0 || !(_Bool)nan)
        return -3.0;

    /* Values cover signed truncation, both unsigned conversion directions
       above LONG_MAX, and narrowing to an exactly representable float. */
    unsigned long high = 0x8000000000000800UL;
    if ((unsigned long)(double)high != high || (int)-f9 != -10 ||
        (float)(i0 - i1) != -1.0f)
        return -4.0;
    float copy = (float)f9;
    copy += f8;
    if (copy++ != 20.0f || --copy != 20.0f)
        return -5.0;
    return i9 + copy + f1;
}

int main(void)
{
    float (*scale)(float) = half;
    return probe(1.5f, 11, 12, 13, 14, 15, 16, 17, 18, 19,
                 scale(5.0f), 3.5, 4.5, 5.5, 6.5, 7.5, 8.5,
                 9.5f, 20, 10.5);
}
