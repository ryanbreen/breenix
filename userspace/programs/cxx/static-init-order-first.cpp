void record_constructor(const char *event);
void record_destructor(const char *event);

namespace {
struct First {
    First() { record_constructor("construct-first\n"); }
    ~First() { record_destructor("destroy-first\n"); }
};

// This object is linked after Second but has the earlier initialization priority.
__attribute__((init_priority(101))) First first;
}
