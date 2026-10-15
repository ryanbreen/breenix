#define _POSIX_C_SOURCE 200809L

#include <math.h>
#include <stdio.h>

#include "app.h"

int cmd_math(int argc, char **argv)
{
    (void)argc;
    (void)argv;
    /* volatile keeps the compiler from folding the calls into constants. */
    volatile double two = 2.0, one = 1.0, ten = 10.0, seven_half = 7.5, neg = -1.5;
    volatile double three = 3.0, four = 4.0, cube = 27.0, half = 2.5;
    printf("sqrt=%.6f sin=%.6f cos=%.6f exp=%.6f log=%.6f\n",
           sqrt(two), sin(one), cos(0.0 * one), exp(one), log(ten));
    printf("pow=%.6f fmod=%.6f floor=%.6f ceil=%.6f\n",
           pow(two, ten), fmod(seven_half, two), floor(neg), ceil(neg));
    printf("pi=%.6f hypot=%.6f cbrt=%.6f lround=%ld\n",
           atan2(one, one) * 4.0, hypot(three, four), cbrt(cube), lround(half));
    return 0;
}
