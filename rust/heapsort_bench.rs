use std::env;
use std::time::Instant;

fn step(x: u64) -> u64 {
    x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)
}

// Max-heap, exclusive end; same comparisons and swaps as C++ implementation.
fn sift_down(a: &mut [u64], mut root: usize, end: usize) {
    if end < 2 { return; }
    while root <= (end - 2) / 2 {
        let mut child = 2 * root + 1;
        if child + 1 < end && a[child] < a[child + 1] { child += 1; }
        if a[root] >= a[child] { return; }
        a.swap(root, child);
        root = child;
    }
}

fn heap_sort(a: &mut [u64]) {
    let n = a.len();
    if n < 2 { return; }
    let mut start = n / 2;
    while start > 0 {
        start -= 1;
        sift_down(a, start, n);
    }
    let mut end = n;
    while end > 1 {
        end -= 1;
        a.swap(0, end);
        sift_down(a, 0, end);
    }
}

fn main() {
    let n: usize = env::args().nth(1).expect("size").parse().expect("integer");
    let mut a = Vec::with_capacity(n);
    let mut x = 1u64;
    for _ in 0..n {
        x = step(x);
        a.push(x);
    }
    let start = Instant::now();
    heap_sort(&mut a);
    let elapsed = start.elapsed();
    assert!(a.windows(2).all(|w| w[0] <= w[1]), "not sorted");
    let mut hash = 14695981039346656037u64;
    for v in a { hash = (hash ^ v).wrapping_mul(1099511628211); }
    println!("{} {}", elapsed.as_nanos(), hash);
}
