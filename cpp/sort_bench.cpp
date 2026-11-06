#include <algorithm>
#include <chrono>
#include <cstdint>
#include <iostream>
#include <string>
#include <vector>
using u64 = std::uint64_t;
static inline u64 step(u64 x) { return x * 6364136223846793005ULL + 1442695040888963407ULL; }
int main(int argc, char** argv) {
  if (argc != 2) return 2;
  const std::size_t n = std::stoull(argv[1]);
  std::vector<u64> values;
  values.reserve(n);
  u64 x = 1;
  for(std::size_t i=0;i<n;++i) { x=step(x); values.push_back(x); }
  const auto start=std::chrono::steady_clock::now();
  std::sort(values.begin(),values.end());
  const auto finish=std::chrono::steady_clock::now();
  if(!std::is_sorted(values.begin(),values.end())) return 3;
  // FNV-1a style order-dependent checksum (64-bit wrapping arithmetic).
  u64 hash=14695981039346656037ULL;
  for(u64 v:values) { hash ^= v; hash *= 1099511628211ULL; }
  const auto ns=std::chrono::duration_cast<std::chrono::nanoseconds>(finish-start).count();
  std::cout << ns << " " << hash << "\n";
}
