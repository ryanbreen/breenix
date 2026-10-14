#!/usr/bin/env python3
"""Derive Breenix's own LCG coefficients with exact integer square roots.

The fractional bits of sqrt(2) and sqrt(3) provide dense implementation-selected
words. For modulus 2^w, an odd increment and a multiplier 1 modulo 4 give full
period; force those low bits explicitly. No libc source or parameter table is used.
"""
from math import isqrt

for width in (64, 32):
    mask = (1 << width) - 1
    multiplier = (isqrt(2 << (2 * width)) & mask & ~3) | 1
    increment = (isqrt(3 << (2 * width)) & mask) | 1
    print(f'{width}: multiplier=0x{multiplier:0{width//4}x}, increment=0x{increment:0{width//4}x}')
