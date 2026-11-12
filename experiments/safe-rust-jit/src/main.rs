#![forbid(unsafe_code)]
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
        let mut header = [0u8; 12];
        match input.read_exact(&mut header) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let count = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let mut value = u64::from_le_bytes(header[4..12].try_into().unwrap());
        for _ in 0..count { value = compute(value); }
        output.write_all(&value.to_le_bytes())?;
        output.flush()?;
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
        let mut request = [0u8;12];
        request[..4].copy_from_slice(&batch.to_le_bytes());
        request[4..].copy_from_slice(&7u64.to_le_bytes());
        let mut response = [0u8;8];
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
                tx.write_all(&request)?;
                rx.read_exact(&mut response)?;
                black_box(u64::from_le_bytes(response));
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
