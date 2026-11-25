extern long outer_data[];
extern void outer_function(void);

long choice = 7;
long *absolute_slot = outer_data + 1;
static long local_data;
long *relative_slot = &local_data;

static void preinit_first(void) { local_data = 1; }
static void preinit_second(void) { local_data += 2; }
static void main_init(void) { local_data += 4; }

__attribute__((used, section(".preinit_array")))
static void (*const preinit[])(void) = {preinit_first, preinit_second};

__attribute__((used, section(".init_array")))
static void (*const init[])(void) = {main_init};

void _start(void) { outer_function(); }
