/* Black-box input for elfcheck: clang compiles this file to x86-64 ELF
 * relocatable objects, and elfcheck must accept every one. It exercises
 * defined and undefined globals, weak, hidden and common symbols, static and
 * external thread-local variables, merged string literals and a table of
 * absolute function addresses. */

extern int external_value;
extern int external_function(int);
extern __attribute__((weak)) int weak_function(int);
extern _Thread_local int external_tls;

int tentative_buffer[16];
__attribute__((visibility("hidden"))) int hidden_counter = 3;
static int file_static = 7;
static _Thread_local int static_tls;
_Thread_local long defined_tls = 11;

static const char *const messages[] = {"first", "second", "first"};

static int add_one(int value)
{
    return value + 1;
}

int (*const operations[])(int) = {add_one, external_function};

int combine(int index)
{
    static_tls += index;
    defined_tls += static_tls;
    int total = external_value + hidden_counter + file_static + external_tls;
    if (weak_function)
        total += weak_function(index);
    total += operations[index & 1](index);
    tentative_buffer[index & 15] = total;
    return total + messages[index % 3][0];
}
