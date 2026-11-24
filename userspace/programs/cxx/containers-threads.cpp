#include "probe.hpp"

#include <algorithm>
#include <map>
#include <mutex>
#include <thread>
#include <vector>

int main(int argc, char **argv) {
    const int fd = pipe_fd(argc, argv);
    if (fd < 0) {
        return 1;
    }

    std::mutex mutex;
    std::vector<int> values;
    std::map<int, unsigned> counts;
    auto insert = [&](int first) {
        for (int value = first; value < first + 3; ++value) {
            const std::lock_guard lock(mutex);
            values.push_back(value);
            ++counts[value];
        }
    };

    std::thread first(insert, 1);
    std::thread second(insert, 2);
    first.join();
    second.join();
    if (!write_event(fd, "threads-joined\n")) {
        return 1;
    }

    std::sort(values.begin(), values.end());
    if (values != std::vector{1, 2, 2, 3, 3, 4} || !write_event(fd, "vector-verified\n")) {
        return 1;
    }
    const std::map<int, unsigned> expected{{1, 1}, {2, 2}, {3, 2}, {4, 1}};
    return counts == expected && write_event(fd, "map-verified\n") ? 0 : 1;
}
