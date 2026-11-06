#include <algorithm>
#include <cstdint>
#include <iostream>
#include <numeric>
#include <string>
#include <vector>
using u64 = std::uint64_t;
static u64 step(u64 x) { return x * 6364136223846793005ULL + 1442695040888963407ULL; }
int main(int argc, char** argv) {
  if (argc != 2) return 2;
  std::string test=argv[1];
  if(test=="integers") {
    u64 x=1, sum=0;
    for(int i=0;i<5000000;i++){ x=step(x); sum+=x>>32; }
    std::cout<<sum<<"\n";
  } else if(test=="primes") {
    constexpr int n=400000; std::vector<bool> prime(n+1,true);
    prime[0]=prime[1]=false;
    for(int p=2;p*p<=n;p++) if(prime[p]) for(int j=p*p;j<=n;j+=p) prime[j]=false;
    u64 sum=0;for(int i=2;i<=n;i++) if(prime[i])sum+=i;
    std::cout<<sum<<"\n";
  } else if(test=="sorting") {
    std::vector<u64> values;values.reserve(120000);u64 x=1;
    for(int i=0;i<120000;i++){x=step(x);values.push_back(x);}
    std::sort(values.begin(),values.end());
    u64 sum=0;for(auto v:values)sum+=v;
    std::cout<<sum<<"\n";
  } else return 2;
}
