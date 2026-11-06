use std::env;
use std::time::Instant;
fn step(x:u64)->u64 { x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) }
fn main() {
    let n:usize=env::args().nth(1).expect("size").parse().expect("integer");
    let mut values=Vec::with_capacity(n);
    let mut x=1u64;
    for _ in 0..n {x=step(x);values.push(x);}
    let start=Instant::now();
    values.sort_unstable();
    let elapsed=start.elapsed();
    assert!(values.windows(2).all(|w|w[0]<=w[1]),"not sorted");
    let mut hash=14695981039346656037u64;
    for v in values {hash=(hash^v).wrapping_mul(1099511628211);}
    println!("{} {}",elapsed.as_nanos(),hash);
}
