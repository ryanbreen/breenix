/* AMD64 psABI §10.3 local-dynamic template, matching X2's PT_TLS. */
static _Thread_local struct {
    unsigned long a, b, c, d, e, f;
} block;

void *tls_address(void) { return &block; }
