/* C17 corpus: o2-expect.
 * The abstract machine fixes every printed value, so the -O0 and -O2 builds
 * must print the same text. The lines cover constant propagation, dead
 * branches, common subexpressions, volatile accesses and values live across
 * calls. Volatile reads feed the later sections so -O2 cannot fold them away,
 * and longjmp makes volatile objects observable: it keeps the values of
 * volatile automatic objects only (7.13.2.1p3).
 * Clauses: N1570 5.1.2.3p2, 5.1.2.3p4, 5.1.2.3p6, 6.2.5p9, 6.3.1.3p2, 6.5.5p6, 6.5.7p4,
 *   6.5.13p4, 6.5.14p4, 6.5.15p4, 6.8.4.2p4, 6.8.5.2p1, 7.13.1.1p4, 7.13.2.1p3-4;
 *   C17 5.1.2.3p6.
 * Expectations: o2-expect.expect, written by hand from these clauses.
 */
#include <limits.h>
#include <setjmp.h>
#include <stdio.h>

static volatile int source = 3;
static volatile int inputs[16] = { 3, 7, 11, 15, 19, 23, 27, 31, 35, 39, 43, 47, 51, 55, 59, 63 };
static volatile double scales[8] = { 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 6.5 };

/* Constant propagation: unsigned arithmetic wraps (6.2.5p9), division
 * truncates toward zero (6.5.5p6), conversion to unsigned char reduces modulo
 * UCHAR_MAX + 1 (6.3.1.3p2), and 1u << 15 is 32768 (6.5.7p4). */
static void constant(void)
{
    int a = 6;
    int b = a * 7;
    unsigned wrapped = UINT_MAX;
    wrapped += 2u;
    long long wide = 0x7fffffffLL * 2;
    int quotient = -7 / 2;
    int remainder = -7 % 2;
    unsigned char narrow = (unsigned char)(UCHAR_MAX + 45);
    unsigned shifted = 1u << 15;
    int total = 0;
    for (int i = 0; i < 10; ++i)
        total += i * 3;
    int cell = 5;
    int *through = &cell;
    *through += 1;
    printf("constant %d %u %lld %d %d %u %u %d %d\n", b, wrapped, wide, quotient, remainder,
           (unsigned)narrow, shifted, total, cell * 2);
}

static int calls;
static int touched;

static int touch(int value)
{
    ++calls;
    touched += value;
    return value;
}

/* Dead branches: only the taken arms call touch. && and || skip their right
 * operand (6.5.13p4, 6.5.14p4), ?: evaluates one arm (6.5.15p4), switch jumps
 * to its case and falls through (6.8.4.2p4), and do runs its body once
 * (6.8.5.2p1). */
static void dead(void)
{
    int k = 3;
    int limit = 2;
    limit *= 3;
    if (0)
        touch(1000);
    if (limit != 6)
        touch(2000);
    if (k > 5)
        touch(3);
    else
        touch(4);
    switch (k) {
    case 1:
        touch(10);
        break;
    case 3:
        touch(30);
        /* fall through */
    case 4:
        touch(40);
        break;
    default:
        touch(99);
        break;
    }
    if (0 && touch(5))
        touch(6);
    if (1 || touch(7))
        k += 1;
    int chosen = k > 0 ? 8 : touch(9);
    while (0)
        touch(11);
    do
        touch(12);
    while (0);
    printf("dead %d %d %d %d\n", calls, touched, k, chosen);
}

static int global = 4;

static void bump(void)
{
    global += 10;
}

/* A store through q may change *p, so *p * 3 is computed again. */
static int reload(int *p, int *q)
{
    int first = *p * 3;
    *q = 9;
    int second = *p * 3;
    return first * 100 + second;
}

/* bump changes global, so global * global is computed again. */
static int across_call(void)
{
    int before = global * global;
    bump();
    int after = global * global;
    return before + after;
}

/* Common subexpressions: reuse is correct only where nothing in between can
 * change the operands. */
static void cse(void)
{
    int x = 2;
    int same = reload(&x, &x);
    int y = 2;
    int z = 0;
    int apart = reload(&y, &z);
    int base = source;
    int left = (base * 5 + 1) * (base * 5 + 1);
    int right = base * 5 + 1 - base;
    int changed = across_call();
    printf("cse %d %d %d %d %d %d\n", same, x, apart, changed, left, right);
}

static jmp_buf target;

static void jump(void)
{
    longjmp(target, 2);
}

/* setjmp appears as a whole switch controlling expression (7.13.1.1p4).
 * After longjmp it returns 2 (7.13.2.1p4), and the volatile locals keep the
 * values stored before the call (7.13.2.1p3). */
static int after_longjmp(void)
{
    volatile int kept = 1;
    volatile int stage = 0;
    switch (setjmp(target)) {
    case 0:
        kept = 5;
        stage = 1;
        jump();
        break;
    case 2:
        return kept * 100 + stage * 10 + 2;
    default:
        break;
    }
    return -1;
}

/* Every volatile access is performed (5.1.2.3p6): 1000 increments, then two
 * separate reads of the same object. */
static unsigned ticks(void)
{
    volatile unsigned counter = 0;
    for (int i = 0; i < 1000; ++i)
        counter++;
    unsigned first = counter;
    unsigned second = counter;
    return first + second;
}

static void volatiles(void)
{
    int jumped = after_longjmp();
    unsigned counted = ticks();
    printf("volatile %d %u\n", jumped, counted);
}

/* Uses many registers of its own: with seed 3 it returns -90. */
static int churn(int seed)
{
    int a = seed + 1;
    int b = a * 2;
    int c = b + a;
    int d = c * c;
    int e = d - b;
    int f = e / 3;
    int g = f % 7;
    int h = g + a + b + c;
    return h * 2 - d;
}

/* Sixteen integers and eight doubles are read from volatile objects before
 * the call, in order, so all of them stay live across it. inputs[i] is
 * 4i + 3 and scales[i] is 3 + i / 2, so the weighted sums are exact:
 * sum (i + 1)(4i + 3) for i < 16 is 5848, and sum (i + 1)(3 + i / 2) for
 * i < 8 is 192. */
static void spill(void)
{
    int v0 = inputs[0], v1 = inputs[1], v2 = inputs[2], v3 = inputs[3];
    int v4 = inputs[4], v5 = inputs[5], v6 = inputs[6], v7 = inputs[7];
    int v8 = inputs[8], v9 = inputs[9], v10 = inputs[10], v11 = inputs[11];
    int v12 = inputs[12], v13 = inputs[13], v14 = inputs[14], v15 = inputs[15];
    double d0 = scales[0], d1 = scales[1], d2 = scales[2], d3 = scales[3];
    double d4 = scales[4], d5 = scales[5], d6 = scales[6], d7 = scales[7];
    int churned = churn(source);
    long long ints = v0 + 2LL * v1 + 3LL * v2 + 4LL * v3 + 5LL * v4 + 6LL * v5 + 7LL * v6
        + 8LL * v7 + 9LL * v8 + 10LL * v9 + 11LL * v10 + 12LL * v11 + 13LL * v12
        + 14LL * v13 + 15LL * v14 + 16LL * v15;
    double doubles = d0 + 2 * d1 + 3 * d2 + 4 * d3 + 5 * d4 + 6 * d5 + 7 * d6 + 8 * d7;
    printf("spill %lld %d %d\n", ints, churned, (int)doubles);
}

int main(void)
{
    constant();
    dead();
    cse();
    volatiles();
    spill();
    return 0;
}
