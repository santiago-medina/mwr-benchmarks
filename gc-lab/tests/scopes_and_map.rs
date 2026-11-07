use gc_lab::operations::{map_resolving, map_stable};
use gc_lab::{Error, Heap, Model};
use std::panic::{catch_unwind, AssertUnwindSafe};
const MODELS: [Model; 3] = [Model::Stable, Model::Compact, Model::Indirect];

#[test]
fn scoped_root_slots_are_reused_without_reviving_escaped_tokens() {
    for model in MODELS {
        let mut heap = Heap::new(model);
        let x = heap.allocate(1);
        let outer = heap.root(x).unwrap();
        let old = heap.scope(|s| s.root(x).unwrap());
        for _ in 0..10_000 {
            heap.scope(|s| {
                let current = s.root(x).unwrap();
                assert_eq!(s.resolve(old), Err(Error::InvalidRoot));
                assert_eq!(s.unroot(old), Err(Error::InvalidRoot));
                assert_eq!(s.resolve(current), Ok(x));
            });
        }
        assert_eq!(heap.root_slot_count(), 2);
        assert_eq!(heap.resolve(outer), Ok(x));
    }
}

#[test]
fn scopes_release_on_success_error_and_panic() {
    for model in MODELS {
        let mut heap = Heap::new(model);
        let outer = heap.allocate(1);
        let outer_root = heap.root(outer).unwrap();
        let escaped = heap.scope(|s| {
            let x = s.allocate(2);
            s.root(x).unwrap()
        });
        assert_eq!(heap.resolve(escaped), Err(Error::InvalidRoot));
        let result: Result<(), Error> = heap.scope(|s| {
            let x = s.allocate(3);
            s.root(x)?;
            Err(Error::InvalidHandle)
        });
        assert!(result.is_err());
        let result = catch_unwind(AssertUnwindSafe(|| {
            heap.scope(|s| {
                let x = s.allocate(4);
                s.root(x).unwrap();
                s.scope(|inner| {
                    let x = inner.allocate(5);
                    inner.root(x).unwrap();
                    panic!("guest panic");
                });
            })
        }));
        assert!(result.is_err());
        assert_eq!(heap.live_roots(), 1);
        assert_eq!(heap.collect().live_objects, 1);
        assert_eq!(heap.value(heap.resolve(outer_root).unwrap()), Ok(1));
    }
}

#[test]
fn scoped_allocation_stress_preserves_graph_in_all_models() {
    for model in MODELS {
        let mut heap = Heap::new(model);
        heap.set_stress(true);
        heap.scope(|s| {
            let first = s.allocate(0);
            let root = s.root(first).unwrap();
            for i in 1..=128 {
                let child = s.allocate(i);
                let parent = s.resolve(root).unwrap();
                s.link(parent, child).unwrap();
            }
            assert_eq!(s.collect().live_objects, 129);
            let sum: i64 = s
                .edges(s.resolve(root).unwrap())
                .unwrap()
                .into_iter()
                .map(|h| s.value(h).unwrap())
                .sum();
            assert_eq!(sum, 8256);
        });
        assert_eq!(heap.live_roots(), 0);
        assert_eq!(heap.collect().live_objects, 0);
    }
}

#[test]
fn maps_match_scalar_oracle_under_reentrant_gc_and_allocation_stress() {
    for model in MODELS {
        for stable_path in [false, true] {
            if stable_path && model == Model::Compact {
                continue;
            }
            let mut heap = Heap::new(model);
            heap.set_stress(true);
            let input = heap.allocate(0);
            let input_root = heap.root(input).unwrap();
            for i in 0..64 {
                let child = heap.allocate(i);
                heap.link(heap.resolve(input_root).unwrap(), child).unwrap();
            }
            let callback = |h: &mut Heap, item| {
                h.call(item, |h, item| {
                    h.allocate(-123);
                    h.call(item, |h, item| {
                        h.collect();
                        Ok(h.value(h.resolve(item)?)? * 3 + 1)
                    })
                })
            };
            let result = if stable_path {
                map_stable(&mut heap, input_root, callback)
            } else {
                map_resolving(&mut heap, input_root, callback)
            }
            .unwrap();
            let output_root = heap.root(result).unwrap();
            heap.collect();
            let actual: Vec<_> = heap
                .edges(heap.resolve(output_root).unwrap())
                .unwrap()
                .into_iter()
                .map(|h| heap.value(h).unwrap())
                .collect();
            assert_eq!(actual, (0..64).map(|i| i * 3 + 1).collect::<Vec<_>>());
            assert_eq!(heap.live_roots(), 2);
            heap.unroot(input_root).unwrap();
            heap.unroot(output_root).unwrap();
            assert_eq!(heap.collect().live_objects, 0);
        }
    }
}

#[test]
fn map_error_and_panic_release_output_and_item_roots() {
    for model in MODELS {
        for stable_path in [false, true] {
            if stable_path && model == Model::Compact {
                continue;
            }
            for panic_in_callback in [false, true] {
                let mut heap = Heap::new(model);
                let input = heap.allocate(0);
                let root = heap.root(input).unwrap();
                for i in 0..4 {
                    let x = heap.allocate(i);
                    heap.link(input, x).unwrap();
                }
                heap.set_stress(true);
                let result = catch_unwind(AssertUnwindSafe(|| {
                    let mut calls = 0;
                    let callback = |h: &mut Heap, _| {
                        h.collect();
                        calls += 1;
                        if calls == 3 {
                            if panic_in_callback {
                                panic!("guest callback panic");
                            }
                            Err(Error::InvalidHandle)
                        } else {
                            Ok(42)
                        }
                    };
                    if stable_path {
                        map_stable(&mut heap, root, callback)
                    } else {
                        map_resolving(&mut heap, root, callback)
                    }
                }));
                if panic_in_callback {
                    assert!(result.is_err());
                } else {
                    assert_eq!(result.unwrap(), Err(Error::InvalidHandle));
                }
                assert_eq!(heap.live_roots(), 1);
                assert_eq!(heap.collect().live_objects, 5);
            }
        }
    }
}

#[test]
fn indirect_tokens_survive_repeated_compaction_and_reuse() {
    let mut h = Heap::new(Model::Indirect);
    let keeper = h.allocate(7);
    let root = h.root(keeper).unwrap();
    for _ in 0..64 {
        let old = h.allocate(100);
        h.collect();
        let new = h.allocate(200);
        assert_eq!(h.value(old), Err(Error::InvalidHandle));
        assert_eq!(h.value(new), Ok(200));
        assert_eq!(h.resolve(root), Ok(keeper));
        assert_eq!(h.value(keeper), Ok(7));
    }
}

#[test]
fn stress_exposes_unrooted_temporaries_and_empty_map_works() {
    for model in MODELS {
        let mut h = Heap::new(model);
        h.set_stress(true);
        let lost = h.allocate(9);
        h.allocate(10);
        assert_eq!(h.value(lost), Err(Error::InvalidHandle));
        let input = h.allocate(0);
        let root = h.root(input).unwrap();
        let output = map_resolving(&mut h, root, |_, _| panic!("empty callback")).unwrap();
        assert!(h.edges(output).unwrap().is_empty());
        assert_eq!(h.live_roots(), 1);
    }
}
