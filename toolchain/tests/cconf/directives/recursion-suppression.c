#define foo foo
#define bar baz
#define baz bar
#define self(x) x self(x)
#define AGAIN(x) x
#define LATER AGAIN(LATER)
foo bar baz self(1) LATER
