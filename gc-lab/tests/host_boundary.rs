use gc_lab::{host, Error, Heap, Model};
const MODELS: [Model; 3] = [Model::Stable, Model::Compact, Model::Indirect];

#[test]
fn callback_result_keeps_capture_alive_and_entire_cycle_is_reclaimed() {
    for model in MODELS {
        let mut h = Heap::new(model);
        h.set_stress(true);
        let node = host::create_node_with_listener(&mut h).unwrap();
        let root = h.root(node).unwrap();
        let result = host::dispatch(&mut h, root, 3).unwrap();
        let result_root = h.root(result).unwrap();
        assert_eq!(h.live_roots(), 2);
        h.unroot(root).unwrap();
        assert_eq!(h.collect().live_objects, 6); // node, listener, four results
        let result = h.resolve(result_root).unwrap();
        assert_eq!(h.value(result), Ok(3));
        let mut current = result;
        for expected in (0..=3).rev() {
            assert_eq!(h.value(current), Ok(expected));
            let node = h.edge(current, 0).unwrap();
            let listener = h.edge(node, 0).unwrap();
            assert_eq!(h.edge(listener, 0).unwrap(), node);
            if expected > 0 {
                current = h.edge(current, 1).unwrap();
            }
        }
        h.unroot(result_root).unwrap();
        assert_eq!(h.collect().live_objects, 0);
    }
}

#[test]
fn detached_listener_survives_only_when_externally_reachable() {
    for model in MODELS {
        let mut h = Heap::new(model);
        h.set_stress(true);
        let node = host::create_node_with_listener(&mut h).unwrap();
        let root = h.root(node).unwrap();
        let callback = h.edge(node, 0).unwrap();
        let callback_root = h.root(callback).unwrap();
        host::remove_listener(&mut h, root).unwrap();
        h.unroot(root).unwrap();
        assert_eq!(h.collect().live_objects, 2);
        h.unroot(callback_root).unwrap();
        assert_eq!(h.collect().live_objects, 0);
    }
}

#[test]
fn partial_callback_results_are_reclaimed_after_budget_failure_and_retry_works() {
    for model in MODELS {
        for stress in [false, true] {
            let mut h = Heap::new(model);
            h.set_stress(stress);
            let node = host::create_node_with_listener(&mut h).unwrap();
            let root = h.root(node).unwrap();
            h.set_object_limit(Some(4)); // fails after two nested results
            assert_eq!(host::dispatch(&mut h, root, 2), Err(Error::ObjectLimit));
            assert_eq!(h.live_roots(), 1);
            assert_eq!(h.collect().live_objects, 2);
            let result = host::dispatch(&mut h, root, 0).unwrap();
            let output = h.root(result).unwrap();
            assert_eq!(h.value(h.resolve(output).unwrap()), Ok(0));
            h.unroot(root).unwrap();
            h.unroot(output).unwrap();
            assert_eq!(h.collect().live_objects, 0);
        }
    }
}

#[test]
fn partial_host_construction_releases_its_roots() {
    for model in MODELS {
        let mut h = Heap::new(model);
        h.set_stress(true);
        h.set_object_limit(Some(1));
        assert_eq!(
            host::create_node_with_listener(&mut h),
            Err(Error::ObjectLimit)
        );
        assert_eq!(h.live_roots(), 0);
        assert_eq!(h.collect().live_objects, 0);
        h.set_object_limit(Some(2));
        assert!(host::create_node_with_listener(&mut h).is_ok());
    }
}

#[test]
fn zero_budget_and_dropped_caller_root_are_rejected() {
    for model in MODELS {
        let mut h = Heap::new(model);
        h.set_object_limit(Some(0));
        assert_eq!(h.try_allocate(0), Err(Error::ObjectLimit));
        h.set_object_limit(None);
        let node = host::create_node_with_listener(&mut h).unwrap();
        let root = h.root(node).unwrap();
        assert_eq!(host::dispatch(&mut h, root, 33), Err(Error::ReentryLimit));
        h.unroot(root).unwrap();
        assert_eq!(host::dispatch(&mut h, root, 0), Err(Error::InvalidRoot));
        assert_eq!(h.live_roots(), 0);
    }
}
