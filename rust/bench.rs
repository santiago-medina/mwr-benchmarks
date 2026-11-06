use std::env;
fn step(x:u64)->u64 { x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) }
fn main() {
 let test=env::args().nth(1).expect("algorithm");
 let out:u64=match test.as_str() {
  "integers" => {let (mut x,mut sum)=(1u64,0u64);for _ in 0..5_000_000 {x=step(x);sum=sum.wrapping_add(x>>32);}sum},
  "primes" => {let n=400_000usize;let mut prime=vec![true;n+1];prime[0]=false;prime[1]=false;
    let mut p=2;while p*p<=n {if prime[p] {let mut j=p*p;while j<=n {prime[j]=false;j+=p;}}p+=1;}
    prime.iter().enumerate().filter(|(_,v)| **v).fold(0u64,|acc,(i,_)|acc+i as u64)},
  "sorting" => {let mut a=Vec::with_capacity(120_000);let mut x=1u64;
    for _ in 0..120_000 {x=step(x);a.push(x);}a.sort_unstable();
    a.iter().fold(0u64,|acc,v|acc.wrapping_add(*v))},
  _ => panic!("unknown algorithm"),
 };
 println!("{out}");
}
