use gc_lab::{Error, Heap, Model};

fn models() -> [Model; 3] {
    [Model::Stable, Model::Compact, Model::Indirect]
}

#[test]
fn rooted_graph_survives_reentrant_callbacks() {
    for model in models() {
        let mut h = Heap::new(model);
        h.allocate(-1); // Force relocation of the live graph.
        let parent = h.allocate(10);
        let child = h.allocate(20);
        h.link(parent, child).unwrap();
        h.link(child, parent).unwrap(); // Reachable cycle.
        let root = h.root(parent).unwrap();
        h.call(root, |h, root| {
            h.collect();
            h.call(root, |h, root| {
                h.allocate(999);
                h.collect();
                let parent = h.resolve(root).unwrap();
                assert_eq!(h.value(parent), Ok(10));
                let child = h.edges(parent).unwrap()[0];
                assert_eq!(h.value(child), Ok(20));
                assert_eq!(h.edges(child).unwrap()[0], parent);
            });
        });
    }
}

#[test]
fn dead_cycles_and_roots_are_reclaimed() {
    for model in models() {
        let mut h = Heap::new(model);
        let a = h.allocate(1);
        let b = h.allocate(2);
        h.link(a, b).unwrap();
        h.link(b, a).unwrap();
        let root = h.root(a).unwrap();
        h.unroot(root).unwrap();
        assert_eq!(h.resolve(root), Err(Error::InvalidRoot));
        let stats = h.collect();
        assert_eq!((stats.freed_objects, stats.live_objects), (2, 0));
        assert_eq!(h.value(a), Err(Error::InvalidHandle));
    }
}

#[test]
fn reused_slots_never_resurrect_a_stale_handle() {
    for model in models() {
        let mut h = Heap::new(model);
        let old = h.allocate(1);
        h.collect();
        let new = h.allocate(2);
        assert_ne!(old, new);
        assert_eq!(h.value(old), Err(Error::InvalidHandle));
        assert_eq!(h.value(new), Ok(2));
    }
}

#[test]
fn stable_handles_survive_gc_but_compact_handles_need_resolution() {
    for model in models() {
        let mut h = Heap::new(model);
        h.allocate(-1);
        let raw = h.allocate(42);
        let root = h.root(raw).unwrap();
        let stats = h.collect();
        assert_eq!(h.value(h.resolve(root).unwrap()), Ok(42));
        match model {
            Model::Stable | Model::Indirect => {
                assert_eq!(h.value(raw), Ok(42));
                assert_eq!(stats.refreshed_roots, 0);
            }
            Model::Compact => {
                assert_eq!(h.value(raw), Err(Error::InvalidHandle));
                assert_eq!(stats.moved_objects, 1);
            }
        }
    }
}

#[test]
fn tokens_from_another_heap_are_rejected() {
    for model in models() {
        let mut a = Heap::new(model);
        let mut b = Heap::new(model);
        let x = a.allocate(1);
        let root = a.root(x).unwrap();
        let y = b.allocate(2);
        assert_eq!(b.value(x), Err(Error::InvalidHandle));
        assert_eq!(b.root(x), Err(Error::InvalidHandle));
        assert_eq!(b.link(y, x), Err(Error::InvalidHandle));
        assert_eq!(b.resolve(root), Err(Error::InvalidRoot));
    }
}

#[test]
fn long_graph_uses_no_recursive_marking() {
    for model in models() {
        let mut h = Heap::new(model);
        let first = h.allocate(0);
        let root = h.root(first).unwrap();
        let mut last = first;
        for value in 1..20_000 {
            let next = h.allocate(value);
            h.link(last, next).unwrap();
            last = next;
        }
        assert_eq!(h.collect().live_objects, 20_000);
        h.unroot(root).unwrap();
        assert_eq!(h.collect().freed_objects, 20_000);
    }
}

#[test]
fn callback_error_keeps_explicit_root_valid_until_owner_releases_it() {
    for model in models() {
        let mut h = Heap::new(model);
        let obj = h.allocate(7);
        let root = h.root(obj).unwrap();
        let result: Result<(), &str> = h.call(root, |h, _| {
            h.collect();
            Err("guest exception")
        });
        assert_eq!(result, Err("guest exception"));
        assert_eq!(h.value(h.resolve(root).unwrap()), Ok(7));
        h.unroot(root).unwrap();
        assert_eq!(h.collect().freed_objects, 1);
    }
}

#[test]
fn generated_graph_matches_independent_reachability_oracle() {
    for model in models() {
        for seed in 1..=32_u64 {
            let mut h = Heap::new(model);
            let nodes: Vec<_> = (0..100).map(|v| h.allocate(v)).collect();
            let mut graph = vec![vec![]; 100];
            let mut state = seed;
            for (i, edges) in graph.iter_mut().enumerate() {
                for _ in 0..2 {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let j = (state >> 32) as usize % 100;
                    edges.push(j);
                    h.link(nodes[i], nodes[j]).unwrap();
                }
            }
            let root = h.root(nodes[0]).unwrap();
            let mut seen = [false; 100];
            let mut pending = vec![0];
            while let Some(i) = pending.pop() {
                if !seen[i] {
                    seen[i] = true;
                    pending.extend(graph[i].iter().copied());
                }
            }
            for _ in 0..3 {
                let stats = h.collect();
                assert_eq!(stats.live_objects, seen.iter().filter(|&&v| v).count());
                let mut visited = [false; 100];
                let mut pending = vec![h.resolve(root).unwrap()];
                while let Some(obj) = pending.pop() {
                    let i = h.value(obj).unwrap() as usize;
                    if !visited[i] {
                        visited[i] = true;
                        pending.extend(h.edges(obj).unwrap());
                    }
                }
                assert_eq!(visited, seen);
            }
        }
    }
}
