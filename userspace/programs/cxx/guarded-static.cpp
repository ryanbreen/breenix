#include "probe.hpp"

#include <array>
#include <atomic>
#include <thread>

namespace {
constexpr unsigned worker_count = 8;
std::atomic<bool> start{false};
std::atomic<unsigned> arriving{0};
std::atomic<unsigned> constructions{0};

struct Guarded {
    unsigned value = 0;

    explicit Guarded(int fd) {
        constructions.fetch_add(1, std::memory_order_relaxed);
        // Keep initialization in progress until all workers reach the call site.
        while (arriving.load(std::memory_order_acquire) != worker_count) {
            std::this_thread::yield();
        }
        if (write_event(fd, "construct\n")) {
            value = worker_count;
        }
    }
};

const Guarded &get_static(int fd) {
    // Itanium C++ ABI §3.3.2, one-time dynamic initialization of a local static.
    static Guarded instance(fd);
    return instance;
}
}

int main(int argc, char **argv) {
    const int fd = pipe_fd(argc, argv);
    if (fd < 0) {
        return 1;
    }
    std::array<std::thread, worker_count> workers;
    std::array<const Guarded *, worker_count> seen{};
    for (unsigned i = 0; i < worker_count; ++i) {
        workers[i] = std::thread([&, i] {
            while (!start.load(std::memory_order_acquire)) {
                std::this_thread::yield();
            }
            arriving.fetch_add(1, std::memory_order_release);
            const auto &instance = get_static(fd);
            if (instance.value == worker_count) {
                seen[i] = &instance;
            }
        });
    }
    start.store(true, std::memory_order_release);
    for (auto &worker : workers) {
        worker.join();
    }
    if (constructions.load(std::memory_order_relaxed) != 1 || seen.front() == nullptr) {
        return 1;
    }
    for (const Guarded *instance : seen) {
        if (instance != seen.front()) {
            return 1;
        }
    }
    return write_event(fd, "workers=8\n") ? 0 : 1;
}
