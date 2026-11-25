extern long inner_data;
extern long missing_weak __attribute__((weak));
extern void inner_function(void);

long choice = 17;
long dso_choice = 23;
long outer_data[2];
long *weak_slot = &missing_weak;

static void outer_init(void) { outer_data[0] = inner_data; }

__attribute__((used, section(".init_array")))
static void (*const init[])(void) = {outer_init};

void outer_function(void) {
    outer_data[1] = choice + inner_data + (&missing_weak != 0);
    inner_function();
}
