use std::env;
use std::time::Instant;

fn step(x: u64) -> u64 {
    x.wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
}

// Safe variant: heap length comes from the slice itself,
// not from a second independent 'end' parameter.
#[inline(always)]
fn sift_down(heap: &mut [u64], mut root: usize) {
    let len = heap.len();
    if len < 2 { return; }

    while root <= (len - 2) / 2 {
        let mut child = 2 * root + 1;
        if child + 1 < len && heap[child] < heap[child + 1] {
            child += 1;
        }
        if heap[root] >= heap[child] { return; }
        heap.swap(root, child);
        root = child;
    }
}

fn heapsort(a: &mut [u64]) {
    let n = a.len();
    if n < 2 { return; }
    for start in (0..n/2).rev() {
        sift_down(a, start);
    }
    for end in (1..n).rev() {
        a.swap(0, end);
        sift_down(&mut a[..end], 0);
    }
}

fn main() {
    let n: usize = env::args().nth(1).expect("size").parse().expect("integer");
    let mut a = Vec::with_capacity(n);
    let mut x = 1u64;
    for _ in 0..n { x = step(x); a.push(x); }

    let start = Instant::now();
    heapsort(&mut a);
    let elapsed = start.elapsed();

    assert!(a.windows(2).all(|w| w[0] <= w[1]), "not sorted");
    let mut hash = 14695981039346656037u64;
    for v in a {
        hash = (hash ^ v).wrapping_mul(1099511628211);
    }
    println!("{} {}", elapsed.as_nanos(), hash);
}
