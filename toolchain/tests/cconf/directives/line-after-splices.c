#define TWO \
1
TWO __LINE__
a\
b __LINE__
/* comment
spanning
lines */ __LINE__
#define C 1 /* a comment inside
a directive */ + 1
C __LINE__
