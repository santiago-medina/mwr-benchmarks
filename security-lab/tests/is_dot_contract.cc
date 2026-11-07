// Isolated regression harness for the IsDot contract in Chromium URL handling.
// This is a minimal reproduction of the publicly inspected function logic,
// NOT a build of Chromium or evidence of Chrome exploitability.
#include <cassert>
#include <cstddef>
#include <cstdint>
#include <string>
#include <string_view>
#include <vector>

template <typename CHAR>
inline size_t IsDot(std::basic_string_view<CHAR> spec) {
  if (spec[0] == '.') {
    return 1;
  }
  if (spec.size() >= 3 && spec[0] == '%' && spec[1] == '2' &&
      (spec[2] == 'e' || spec[2] == 'E')) {
    return 3;
  }
  return 0;
}

int main() {
  const std::vector<std::string> cases = {
      ".", ".abc", "%", "%2", "%2e", "%2E", "%2x", "%2eX",
      "%2e%2E", "..", "abc", "%%2e", "%2e/", "%2"};
  for (const auto& s : cases) {
    if (s.empty()) continue;  // actual callers must establish nonempty view
    const size_t len = IsDot(std::string_view(s));
    assert(len == 0 || len == 1 || len == 3);
    assert(len <= s.size());
    if (len == 3) assert(s[2] == 'e' || s[2] == 'E');
  }
  // Exhaustive short-input coverage for % and . prefix cases.
  const std::string alphabet = ".%2eEx/";
  for (char a : alphabet) {
    std::string s(1, a);
    assert(IsDot(std::string_view(s)) <= s.size());
    for (char b : alphabet) {
      s.resize(1);
      s.push_back(b);
      assert(IsDot(std::string_view(s)) <= s.size());
      for (char c : alphabet) {
        s.resize(2);
        s.push_back(c);
        const size_t len = IsDot(std::string_view(s));
        assert(len <= s.size());
        if (len == 3) {
          // The two Chromium uses of path_value[i + 2] are only
          // reachable when this is true.
          volatile char observed = s[2];
          (void)observed;
        }
      }
    }
  }
}
