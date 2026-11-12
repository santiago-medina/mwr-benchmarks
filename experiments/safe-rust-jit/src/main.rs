#![forbid(unsafe_code)]
mod protocol;
use std::{env, hint::black_box, io::{Read, Write}, process::{Command, Stdio}, time::Instant};

fn compute(mut x: u64) -> u64 {
    for _ in 0..64 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        x ^= x >> 17;
    }
    black_box(x)
}
fn worker() -> std::io::Result<()> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let request = match protocol::recv_request(&mut input) {
            Ok(request) => request,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut value = request.seed;
        for _ in 0..request.batch { value = compute(value); }
        protocol::send_response(&mut output, protocol::Response { value })?;
    }
}
fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a,b| a.total_cmp(b));
    samples[samples.len()/2]
}
fn main() -> std::io::Result<()> {
    if env::args().nth(1).as_deref() == Some("--worker") { return worker(); }
    let iterations: usize = env::var("ITERATIONS").ok().and_then(|v|v.parse().ok()).unwrap_or(1000);
    let repetitions: usize = env::var("REPETITIONS").ok().and_then(|v|v.parse().ok()).unwrap_or(9);
    assert!(iterations > 0 && repetitions > 0);
    let exe = env::current_exe()?;
    println!("mode,batch,median_ns_per_request,median_ns_per_compute");
    for batch in [1u32, 16, 256] {
        let mut native = Vec::new();
        let mut ipc = Vec::new();
        let mut child = Command::new(&exe).arg("--worker")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
        let mut tx = child.stdin.take().unwrap();
        let mut rx = child.stdout.take().unwrap();
        let request = protocol::Request { batch, seed: 7 };
        for rep in 0..=repetitions {
            let start = Instant::now();
            let mut result = rep as u64;
            for _ in 0..iterations {
                for _ in 0..batch { result = compute(black_box(result)); }
            }
            black_box(result);
            let n = start.elapsed().as_nanos() as f64 / iterations as f64;
            let start = Instant::now();
            for _ in 0..iterations {
                protocol::send_request(&mut tx, request)?;
                black_box(protocol::recv_response(&mut rx)?.value);
            }
            let i = start.elapsed().as_nanos() as f64 / iterations as f64;
            if rep > 0 { native.push(n); ipc.push(i); }
        }
        drop(tx);
        let status = child.wait()?;
        assert!(status.success());
        for (mode, samples) in [("native",native),("process_roundtrip",ipc)] {
            let ns = median(samples);
            println!("{mode},{batch},{ns:.2},{:.2}",ns / batch as f64);
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn deterministic() { assert_eq!(compute(123), compute(123)); }
    #[test] fn distinct_inputs() { assert_ne!(compute(1), compute(2)); }
}
