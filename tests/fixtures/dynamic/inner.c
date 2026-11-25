extern long choice;

long dso_choice = 29;
long inner_data = 31;
long *dso_slot = &dso_choice;
long *choice_slot = &choice;

static void inner_init(void) { inner_data += 1; }

__attribute__((used, section(".init_array")))
static void (*const init[])(void) = {inner_init};

void inner_function(void) { inner_data += 2; }
