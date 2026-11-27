/* AAELF64 2025Q1 §5.7.11: a named TLS symbol and a local template offset. */
_Alignas(32) _Thread_local unsigned long initial = 73;
static _Thread_local unsigned long block[5];

unsigned long *initial_address(void) { return &initial; }
unsigned long *tls_address(void) { return block; }
