use gc_lab::{
    dual::{self, Domain, Id, Runtime},
    Error, Model,
};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn configurations() -> [(Model, Model); 4] {
    [
        (Model::Stable, Model::Stable),
        (Model::Stable, Model::Indirect),
        (Model::Indirect, Model::Stable),
        (Model::Indirect, Model::Indirect),
    ]
}

#[test]
fn roots_in_either_heap_preserve_foreign_cycle_and_both_heaps_reclaim_it() {
    for (dom, js) in configurations() {
        for root_js in [false, true] {
            let mut r = Runtime::new(dom, js).unwrap();
            r.set_stress(true);
            let node = dual::create_cycle(&mut r).unwrap();
            let callback = r.edges(node).unwrap()[0];
            let root = r.root(if root_js { callback } else { node }).unwrap();
            let stats = r.collect();
            assert_eq!((stats.dom.live_objects, stats.js.live_objects), (1, 1));
            assert_eq!(r.edges(callback).unwrap(), vec![node]);
            r.unroot(root).unwrap();
            let before = r.charged_bytes();
            let stats = r.collect();
            assert_eq!((stats.dom.freed_objects, stats.js.freed_objects), (1, 1));
            assert_eq!(stats.reclaimed_charge, before);
            assert_eq!(r.charged_bytes(), 0);
            assert_eq!(r.value(node), Err(Error::InvalidHandle));
            assert_eq!(r.value(callback), Err(Error::InvalidHandle));
        }
    }
}

#[test]
fn independent_compaction_keeps_bridge_ids_and_rejects_reused_ids() {
    for (dom, js) in configurations() {
        let mut r = Runtime::new(dom, js).unwrap();
        let dead_dom = r.allocate(Domain::Dom, 0, 1).unwrap();
        let dead_js = r.allocate(Domain::Js, 0, 1).unwrap();
        let node = dual::create_cycle(&mut r).unwrap();
        let root = r.root(node).unwrap();
        let callback = r.edges(node).unwrap()[0];
        let stats = r.collect();
        assert_eq!(stats.dom.moved_objects, usize::from(dom == Model::Indirect));
        assert_eq!(stats.js.moved_objects, usize::from(js == Model::Indirect));
        assert_eq!(r.resolve(root), Ok(node));
        assert_eq!(r.edges(node).unwrap(), vec![callback]);
        for _ in 0..20 {
            r.allocate(Domain::Dom, 1, 1).unwrap();
            r.allocate(Domain::Js, 2, 1).unwrap();
            r.collect();
            assert_eq!(r.value(dead_dom), Err(Error::InvalidHandle));
            assert_eq!(r.value(dead_js), Err(Error::InvalidHandle));
            assert_eq!(r.edges(callback).unwrap(), vec![node]);
        }
    }
}

#[test]
fn callback_objects_survive_reentrant_collection_in_distinct_heaps() {
    for (dom, js) in configurations() {
        let mut r = Runtime::new(dom, js).unwrap();
        r.set_stress(true);
        let node = dual::create_cycle(&mut r).unwrap();
        let root = r.root(node).unwrap();
        let result = dual::dispatch(&mut r, root, 3).unwrap();
        let output = r.root(result).unwrap();
        r.unroot(root).unwrap();
        let stats = r.collect();
        assert_eq!((stats.dom.live_objects, stats.js.live_objects), (1, 5));
        assert_eq!(r.live_roots(), 1);
        let mut current = r.resolve(output).unwrap();
        for depth in (0..=3).rev() {
            assert_eq!(r.value(current), Ok(depth));
            let edges = r.edges(current).unwrap();
            assert_eq!(edges[0], node);
            if depth > 0 {
                current = edges[1];
            }
        }
        r.unroot(output).unwrap();
        r.collect();
        assert_eq!(r.live_objects(), 0);
    }
}

#[test]
fn removing_listener_does_not_drop_externally_rooted_callback_capture() {
    for (dom, js) in configurations() {
        let mut r = Runtime::new(dom, js).unwrap();
        let node = dual::create_cycle(&mut r).unwrap();
        let callback = r.edges(node).unwrap()[0];
        let root = r.root(callback).unwrap();
        r.clear_edges(node).unwrap();
        r.collect();
        assert_eq!(r.live_objects(), 2);
        r.clear_edges(callback).unwrap();
        r.collect();
        assert_eq!(r.live_objects(), 1);
        assert_eq!(r.value(node), Err(Error::InvalidHandle));
        r.unroot(root).unwrap();
        r.collect();
        assert_eq!(r.live_objects(), 0);
    }
}

#[test]
fn byte_budget_handles_different_payloads_and_failed_edges_without_mutation() {
    let mut r = Runtime::new(Model::Indirect, Model::Indirect).unwrap();
    let charge = Runtime::allocation_charge(16).unwrap();
    r.set_budget(Some(charge));
    let x = r.allocate(Domain::Dom, 1, 16).unwrap();
    let root = r.root(x).unwrap();
    assert_eq!(r.charged_bytes(), charge);
    assert_eq!(r.allocate(Domain::Js, 2, 1), Err(Error::ByteLimit));
    assert_eq!(r.link(x, x), Err(Error::ByteLimit));
    assert!(r.edges(x).unwrap().is_empty());
    assert_eq!(r.charged_bytes(), charge);
    r.set_budget(Some(charge + Runtime::edge_charge()));
    r.link(x, x).unwrap();
    assert_eq!(r.charged_bytes(), charge + Runtime::edge_charge());
    r.unroot(root).unwrap();
    r.collect();
    assert_eq!(r.charged_bytes(), 0);
    assert_eq!(
        r.allocate(Domain::Js, 3, 16 + Runtime::edge_charge() + 1),
        Err(Error::ByteLimit)
    );
    assert_eq!(r.allocate(Domain::Js, 3, usize::MAX), Err(Error::ByteLimit));
    assert!(r.allocate(Domain::Js, 4, 16).is_ok());
}

#[test]
fn byte_failure_in_nested_callback_reclaims_partial_results_and_retry_succeeds() {
    for (dom, js) in configurations() {
        for stress in [false, true] {
            let mut r = Runtime::new(dom, js).unwrap();
            r.set_stress(stress);
            let node = dual::create_cycle(&mut r).unwrap();
            let root = r.root(node).unwrap();
            let base = r.charged_bytes();
            r.set_budget(Some(
                base + 2 * Runtime::allocation_charge(64).unwrap() + 3 * Runtime::edge_charge(),
            ));
            assert_eq!(dual::dispatch(&mut r, root, 2), Err(Error::ByteLimit));
            assert_eq!(r.live_roots(), 1);
            r.collect();
            assert_eq!(r.charged_bytes(), base);
            assert_eq!(r.live_objects(), 2);
            let result = dual::dispatch(&mut r, root, 0).unwrap();
            assert_eq!(r.value(result), Ok(0));
        }
    }
}

#[test]
fn partial_bridge_construction_and_panicking_scope_release_both_domains() {
    for (dom, js) in configurations() {
        let mut r = Runtime::new(dom, js).unwrap();
        r.set_stress(true);
        r.set_budget(Some(Runtime::allocation_charge(16).unwrap()));
        assert_eq!(dual::create_cycle(&mut r), Err(Error::ByteLimit));
        assert_eq!(r.live_roots(), 0);
        r.collect();
        assert_eq!(r.charged_bytes(), 0);
        r.set_budget(None);
        let panic = catch_unwind(AssertUnwindSafe(|| {
            r.scope(|s| {
                let node = dual::create_cycle(s).unwrap();
                let _root = s.root(node).unwrap();
                s.scope(|inner| {
                    let callback = inner.edges(node).unwrap()[0];
                    inner.root(callback).unwrap();
                    inner.collect();
                    panic!("cross-heap panic");
                });
            })
        }));
        assert!(panic.is_err());
        assert_eq!(r.live_roots(), 0);
        r.collect();
        assert_eq!(r.live_objects(), 0);
    }
}

#[test]
fn wrong_runtime_domain_and_direct_compaction_are_rejected() {
    assert!(Runtime::new(Model::Compact, Model::Stable).is_err());
    assert!(Runtime::new(Model::Stable, Model::Compact).is_err());
    let mut a = Runtime::new(Model::Stable, Model::Indirect).unwrap();
    let b = Runtime::new(Model::Stable, Model::Indirect).unwrap();
    let node = dual::create_cycle(&mut a).unwrap();
    let root = a.root(node).unwrap();
    assert_eq!(b.value(node), Err(Error::InvalidHandle));
    assert_eq!(b.resolve(root), Err(Error::InvalidRoot));
    let Id::Dom(h) = node else {
        panic!("expected DOM");
    };
    assert_eq!(a.value(Id::Js(h)), Err(Error::InvalidHandle));
    assert_eq!(dual::dispatch(&mut a, root, 33), Err(Error::ReentryLimit));
    let callback = a.edges(node).unwrap()[0];
    a.clear_edges(callback).unwrap();
    assert_eq!(dual::dispatch(&mut a, root, 0), Err(Error::InvalidHandle));
    assert_eq!(a.live_roots(), 1);
}

#[test]
fn mixed_graphs_match_independent_oracle_across_three_collections() {
    for (dom, js) in configurations() {
        for seed in 1..=32_u64 {
            let mut r = Runtime::new(dom, js).unwrap();
            let ids: Vec<_> = (0..80)
                .map(|i| {
                    r.allocate(
                        if i % 2 == 0 { Domain::Dom } else { Domain::Js },
                        i,
                        i as usize % 17,
                    )
                    .unwrap()
                })
                .collect();
            let mut graph = vec![vec![]; 80];
            let mut random = seed;
            for (index, edges) in graph.iter_mut().enumerate() {
                for _ in 0..2 {
                    random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let target = (random >> 32) as usize % 80;
                    r.link(ids[index], ids[target]).unwrap();
                    edges.push(target);
                }
            }
            let roots = [r.root(ids[0]).unwrap(), r.root(ids[7]).unwrap()];
            let mut expected = [false; 80];
            let mut queue = vec![0, 7];
            while let Some(index) = queue.pop() {
                if !expected[index] {
                    expected[index] = true;
                    queue.extend(graph[index].iter().copied());
                }
            }
            for _ in 0..3 {
                let stats = r.collect();
                let expected_dom = expected
                    .iter()
                    .enumerate()
                    .filter(|(i, live)| **live && i % 2 == 0)
                    .count();
                let expected_js = expected.iter().filter(|&&live| live).count() - expected_dom;
                assert_eq!(
                    (stats.dom.live_objects, stats.js.live_objects),
                    (expected_dom, expected_js)
                );
                for (i, &id) in ids.iter().enumerate() {
                    if expected[i] {
                        assert_eq!(r.value(id), Ok(i as i64));
                        assert_eq!(
                            r.edges(id).unwrap(),
                            graph[i].iter().map(|&j| ids[j]).collect::<Vec<_>>()
                        );
                    } else {
                        assert_eq!(r.value(id), Err(Error::InvalidHandle));
                    }
                }
            }
            for root in roots {
                r.unroot(root).unwrap();
            }
            r.collect();
            assert_eq!(r.live_objects(), 0);
            assert_eq!(r.charged_bytes(), 0);
        }
    }
}

#[test]
fn quota_failure_after_first_cross_edge_does_not_make_a_permanent_root() {
    for (dom, js) in configurations() {
        let mut r = Runtime::new(dom, js).unwrap();
        // Node + callback + first edge fit; the reverse edge does not.
        let charge = Runtime::allocation_charge(16).unwrap()
            + Runtime::allocation_charge(32).unwrap()
            + Runtime::edge_charge();
        r.set_budget(Some(charge));
        assert_eq!(dual::create_cycle(&mut r), Err(Error::ByteLimit));
        assert_eq!(r.live_roots(), 0);
        r.collect();
        assert_eq!(r.live_objects(), 0);
        assert_eq!(r.charged_bytes(), 0);
        r.set_budget(Some(charge + Runtime::edge_charge()));
        let node = dual::create_cycle(&mut r).unwrap();
        let root = r.root(node).unwrap();
        assert_eq!(r.charged_bytes(), charge + Runtime::edge_charge());
        r.unroot(root).unwrap();
        r.collect();
        assert_eq!(r.charged_bytes(), 0);
    }
}

#[test]
fn conservative_foreign_pins_are_not_joint_roots_and_invalid_marks_do_not_sweep() {
    let mut r = Runtime::new(Model::Indirect, Model::Indirect).unwrap();
    let node = gc_lab::dual::create_cycle(&mut r).unwrap();
    r.pin_foreign(node, true).unwrap();
    r.collect();
    assert_eq!(r.live_objects(), 2);
    assert!(r.root_ids().is_empty());
    let mut other = Runtime::new(Model::Indirect, Model::Indirect).unwrap();
    let wrong_heap = other.allocate(Domain::Js, 0, 0).unwrap();
    assert!(r.collect_joint(&[wrong_heap]).is_err());
    assert_eq!(r.live_objects(), 2);
    r.collect_joint(&[node]).unwrap();
    assert_eq!(r.live_objects(), 2);
    r.collect_joint(&[]).unwrap();
    assert_eq!(r.live_objects(), 0);
    assert!(r.pin_foreign(node, true).is_err());
}
