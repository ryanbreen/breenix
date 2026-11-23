extern int entry(void) __attribute__((section(".text.unit_entry"), visibility("hidden")));
int entry(void) { return 42; }
int dispatch(void) __attribute__((weak, visibility("protected"), alias("entry")));

extern int payload __attribute__((weak, visibility("hidden"), section(".data.unit_payload")));
int payload = 42;
extern int mirror __attribute__((weak, alias("payload")));

static int __attribute__((used, noinline)) keep(void) { return 5; }
