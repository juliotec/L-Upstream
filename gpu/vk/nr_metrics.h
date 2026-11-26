// CPU-side NR telemetry. Allocation accounting changes only at allocation/retirement;
// completed GPU samples are accumulated without synchronizing the CPU with the GPU.
#pragma once
#include <atomic>
#include <cstdint>

namespace mkwvk::dlss_nr {
struct Metrics {
    std::atomic<uint64_t> owned_bytes{0}, gpu_ns{0}, samples{0};
    std::atomic<uint64_t> evaluated_frame{UINT64_MAX};
    std::atomic<bool> requested{false}, bypass{false};
    std::atomic<uint64_t> timing_sequence{0};
    // One writer, under the temporal chain lock. Readers never wait for it.
    void sample(uint64_t ns) {
        timing_sequence.fetch_add(1, std::memory_order_acq_rel);
        gpu_ns.fetch_add(ns, std::memory_order_relaxed);
        samples.fetch_add(1, std::memory_order_relaxed);
        timing_sequence.fetch_add(1, std::memory_order_release);
    }
    bool read_timing(uint64_t& ns, uint64_t& count) const {
        const auto before = timing_sequence.load(std::memory_order_acquire);
        if (before & 1) return false;
        ns = gpu_ns.load(std::memory_order_relaxed);
        count = samples.load(std::memory_order_relaxed);
        std::atomic_thread_fence(std::memory_order_acquire);
        return timing_sequence.load(std::memory_order_relaxed) == before;
    }
};
struct MetricsView {
    bool active = false;
    bool timing_valid = false;
    uint64_t owned_bytes = 0, gpu_ns = 0, samples = 0;
};
} // namespace mkwvk::dlss_nr
