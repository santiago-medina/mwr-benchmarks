#include <algorithm>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <string>
#include <vector>

using u64 = std::uint64_t;
static inline u64 step(u64 x) {
    return x * 6364136223846793005ULL + 1442695040888963407ULL;
}

// Max-heap, exclusive end; same comparisons and swaps as Rust implementation.
static void sift_down(std::vector<u64>& a, std::size_t root, std::size_t end) {
    if (end < 2) return;
    while (root <= (end - 2) / 2) {
        std::size_t child = 2 * root + 1;
        if (child + 1 < end && a[child] < a[child + 1]) ++child;
        if (a[root] >= a[child]) return;
        std::swap(a[root], a[child]);
        root = child;
    }
}

static void heap_sort(std::vector<u64>& a) {
    const std::size_t n = a.size();
    if (n < 2) return;
    for (std::size_t start = n / 2; start > 0;) {
        --start;
        sift_down(a, start, n);
    }
    for (std::size_t end = n; end > 1;) {
        --end;
        std::swap(a[0], a[end]);
        sift_down(a, 0, end);
    }
}

int main(int argc, char** argv) {
    if (argc != 2) return 2;
    const std::size_t n = std::stoull(argv[1]);
    std::vector<u64> a;
    a.reserve(n);
    u64 x = 1;
    for (std::size_t i = 0; i < n; ++i) {
        x = step(x);
        a.push_back(x);
    }
    const auto start = std::chrono::steady_clock::now();
    heap_sort(a);
    const auto finish = std::chrono::steady_clock::now();
    if (!std::is_sorted(a.begin(), a.end())) return 3;
    u64 hash = 14695981039346656037ULL;
    for (u64 v : a) {
        hash ^= v;
        hash *= 1099511628211ULL;
    }
    const auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(finish - start).count();
    std::cout << ns << " " << hash << "\n";
}
