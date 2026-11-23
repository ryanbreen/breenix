struct S { unsigned x:1; } s;
void f(void) { (void)sizeof s.x; }
