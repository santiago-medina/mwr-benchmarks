use gc_lab::{
    dual::{self, Domain, Runtime},
    Error, Model,
};

fn record(r: &mut Runtime, dom: Model, js: Model, phase: &str) {
    let stats = r.collect();
    println!(
        "{dom:?},{js:?},{phase},{},{},{},{},{}",
        stats.dom.live_objects,
        stats.js.live_objects,
        r.charged_bytes(),
        stats.reclaimed_charge,
        stats.dom.moved_objects + stats.js.moved_objects
    );
}

fn main() {
    println!(
        "dom_model,js_model,phase,dom_live,js_live,charged_bytes,reclaimed_bytes,moved_objects"
    );
    for dom in [Model::Stable, Model::Indirect] {
        for js in [Model::Stable, Model::Indirect] {
            let mut r = Runtime::new(dom, js).unwrap();
            // Leave holes so compacting arenas demonstrably relocate survivors.
            r.allocate(Domain::Dom, 0, 1).unwrap();
            r.allocate(Domain::Js, 0, 1).unwrap();
            let node = dual::create_cycle(&mut r).unwrap();
            let root = r.root(node).unwrap();
            record(&mut r, dom, js, "rooted_cycle");
            r.set_stress(true);
            let result = dual::dispatch(&mut r, root, 3).unwrap();
            let output = r.root(result).unwrap();
            r.unroot(root).unwrap();
            record(&mut r, dom, js, "object_result");
            r.unroot(output).unwrap();
            record(&mut r, dom, js, "released");
            assert_eq!(r.charged_bytes(), 0);
            let node = dual::create_cycle(&mut r).unwrap();
            let root = r.root(node).unwrap();
            let base = r.charged_bytes();
            r.set_budget(Some(
                base + 2 * Runtime::allocation_charge(64).unwrap() + 3 * Runtime::edge_charge(),
            ));
            assert_eq!(dual::dispatch(&mut r, root, 2), Err(Error::ByteLimit));
            record(&mut r, dom, js, "quota_failure_cleaned");
            assert_eq!(r.charged_bytes(), base);
            let result = dual::dispatch(&mut r, root, 0).unwrap();
            assert_eq!(r.value(result), Ok(0));
            r.unroot(root).unwrap();
            record(&mut r, dom, js, "retry_then_release");
            assert_eq!(r.charged_bytes(), 0);
        }
    }
}
