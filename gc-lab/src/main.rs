use gc_lab::{Heap, Model};
use std::{hint::black_box, time::Instant};

fn main() {
    println!("scenario,repeat,model,objects,live,slots_after_gc,retained_capacity_bytes,table_bytes,gc_us,read_us,moved,edge_rewrites,root_refreshes,table_updates,checksum");
    let models = [Model::Stable, Model::Compact, Model::Indirect];
    for (scenario, stride) in [("sparse", 10), ("dense", 1)] {
        for repeat in 0..9 {
            // Rotate order. This remains a synthetic microbenchmark.
            for offset in 0..3 {
                let model = models[(repeat + offset) % 3];
                let mut h = Heap::new(model);
                let mut roots = Vec::new();
                for value in 0..100_000 {
                    let x = h.allocate(value);
                    if value % stride == 0 {
                        roots.push(h.root(x).unwrap());
                    }
                }
                let head = h.resolve(roots[0]).unwrap();
                for &r in &roots {
                    h.link(h.resolve(r).unwrap(), head).unwrap();
                }
                let start = Instant::now();
                let stats = h.collect();
                let gc_us = start.elapsed().as_micros();
                let handles: Vec<_> = roots.iter().map(|&r| h.resolve(r).unwrap()).collect();
                let start = Instant::now();
                let mut checksum = 0_i64;
                for _ in 0..20 {
                    for &handle in &handles {
                        checksum += black_box(h.value(black_box(handle)).unwrap());
                    }
                }
                let read_us = start.elapsed().as_micros();
                let expected = if stride == 10 {
                    499_950_000
                } else {
                    4_999_950_000
                };
                assert_eq!(checksum, expected * 20);
                println!("{scenario},{repeat},{model:?},100000,{},{},{},{},{gc_us},{read_us},{},{},{},{},{checksum}",
                    stats.live_objects, h.slot_count(), h.retained_capacity_bytes(), h.table_capacity_bytes(),
                    stats.moved_objects, stats.rewritten_edges, stats.refreshed_roots, stats.table_updates);
            }
        }
    }
}
