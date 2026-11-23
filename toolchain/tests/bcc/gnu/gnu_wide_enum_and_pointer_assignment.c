enum flags { LOW = 1, HIGH = 0x80000000U, NEXT };
_Static_assert(NEXT == 0x80000001U, "wide implicit enum value");
_Static_assert((HIGH >> 31) == 1, "wide enumerator is unsigned");

static int response(void) {
    return 42;
}

int main(void) {
    int (*call)(void);
    call = (void *)response;
    return call();
}
