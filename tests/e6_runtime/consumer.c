extern void check_startup(void);

#ifdef LATE
void check_late(void)
#else
__attribute__((constructor))
static void check_dependency(void)
#endif
{
    check_startup();
}

/* Make the dependency explicit even with --as-needed. */
void (*const check_consumer)(void) = check_startup;
