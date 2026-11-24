#include "probe.hpp"

namespace {
constinit int events_fd = -1;
constinit const char *startup_events[2] = {};
constinit size_t startup_count = 0;
}

void record_constructor(const char *event) {
    if (startup_count < 2) {
        startup_events[startup_count] = event;
    }
    ++startup_count;
}

void record_destructor(const char *event) {
    if (events_fd >= 0) {
        write_event(events_fd, event);
    }
}

namespace {
struct Second {
    Second() { record_constructor("construct-second\n"); }
    ~Second() { record_destructor("destroy-second\n"); }
};

// The linker sorts .init_array.* by priority across the two translation units.
// The normal Itanium C++ ABI §3.3.5 exit registrations supply the reverse order.
__attribute__((init_priority(102))) Second second;
}

int main(int argc, char **argv) {
    events_fd = pipe_fd(argc, argv);
    if (events_fd < 0 || startup_count != 2) {
        return 1;
    }
    for (const char *event : startup_events) {
        if (!write_event(events_fd, event)) {
            return 1;
        }
    }
    return write_event(events_fd, "main\n") ? 0 : 1;
}
