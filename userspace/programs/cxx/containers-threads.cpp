#include "probe.hpp"

#include <algorithm>
#include <atomic>
#include <map>
#include <mutex>
#include <thread>
#include <vector>

int main(int argc, char **argv) {
    const int fd = pipe_fd(argc, argv);
    if (fd < 0) {
        return 1;
    }

    constexpr unsigned rounds = 4096;
    std::mutex mutex;
    std::vector<int> values;
    std::map<int, unsigned> counts;
    std::atomic<unsigned> ready{0};
    std::atomic<bool> start{false};
    bool mutex_contended = false;
    auto insert = [&](int first) {
        ready.fetch_add(1, std::memory_order_acq_rel);
        while (!start.load(std::memory_order_acquire)) {
            std::this_thread::yield();
        }
        if (!mutex_contended) {
            return;
        }
        for (unsigned round = 0; round < rounds; ++round) {
            const std::lock_guard lock(mutex);
            for (int value = first; value < first + 3; ++value) {
                values.push_back(value);
                ++counts[value];
            }
        }
    };

    // Hold the mutex until a worker has tried it and both workers are ready.
    std::unique_lock held(mutex);
    std::thread first([&] {
        {
            const std::unique_lock contender(mutex, std::try_to_lock);
            mutex_contended = !contender.owns_lock();
        }
        insert(1);
    });
    std::thread second(insert, 2);
    while (ready.load(std::memory_order_acquire) != 2) {
        std::this_thread::yield();
    }
    held.unlock();
    start.store(true, std::memory_order_release);
    first.join();
    second.join();
    if (!mutex_contended || !write_event(fd, "threads-joined\n")) {
        return 1;
    }

    const std::map<int, unsigned> expected_counts{{1, rounds}, {2, 2 * rounds}, {3, 2 * rounds}, {4, rounds}};
    std::vector<int> expected_values;
    for (const auto &[value, count] : expected_counts) {
        expected_values.insert(expected_values.end(), count, value);
    }
    std::sort(values.begin(), values.end());
    if (values != expected_values || !write_event(fd, "vector-verified\n")) {
        return 1;
    }
    return counts == expected_counts && write_event(fd, "map-verified\n") ? 0 : 1;
}
