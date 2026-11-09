//! Weak references (WEAK_REFS.md): ephemeron marking in the runtime and the observable
//! WeakRef / FinalizationRegistry behaviour in the engine.
use gc_lab::{
    dual::{Domain, Runtime},
    script::Engine,
    Error, Model,
};

#[test]
fn ephemeron_values_live_only_while_holder_and_key_live() {
    for model in [Model::Stable, Model::Indirect] {
        let mut r = Runtime::new(model, model).unwrap();
        let map = r.allocate(Domain::Js, 1, 0).unwrap();
        let map_root = r.root(map).unwrap();
        let k1 = r.allocate(Domain::Js, 2, 0).unwrap();
        let k2 = r.allocate(Domain::Js, 3, 0).unwrap();
        let v2 = r.allocate(Domain::Js, 4, 0).unwrap();
        let cyc = r.allocate(Domain::Js, 5, 0).unwrap();
        let back = r.allocate(Domain::Js, 6, 0).unwrap();
        let weak = r.allocate(Domain::Js, 7, 0).unwrap();
        r.link(back, cyc).unwrap();
        // Chain k1 -> k2 -> v2 (values are the next keys), registered in reverse order
        // so marking has to park values until their keys are found.
        r.set_weak(map, vec![weak], vec![(k2, v2), (k1, k2), (cyc, back)])
            .unwrap();
        let k1_root = r.root(k1).unwrap();
        r.collect();
        for id in [k1, k2, v2] {
            assert!(r.value(id).is_ok(), "chain reachable through k1");
        }
        // A value that references its own key does not keep the key alive.
        assert_eq!(r.value(cyc), Err(Error::InvalidHandle));
        assert_eq!(r.value(back), Err(Error::InvalidHandle));
        assert_eq!(r.value(weak), Err(Error::InvalidHandle));
        r.unroot(k1_root).unwrap();
        r.collect();
        for id in [k1, k2, v2] {
            assert_eq!(r.value(id), Err(Error::InvalidHandle));
        }
        r.unroot(map_root).unwrap();
        r.collect();
        assert_eq!((r.live_objects(), r.charged_bytes()), (0, 0));
    }
}

#[test]
fn weakref_clears_and_finalization_registry_cleans_up() {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.execute(
            "var log = [], kept = {}, wr, wm = new WeakMap(), ws = new WeakSet();
             // Defined outside the IIFE: a closure over its environment would keep `t` live.
             var fr = new FinalizationRegistry(function (h) { log.push(h); });
             (function () {
               var t = {};
               wr = new WeakRef(t);
               var cyc = {};
               wm.set(cyc, { back: cyc });
               wm.set(kept, { v: 42 });
               ws.add({}); ws.add(kept);
               fr.register({}, 'gone');
               fr.register(kept, 'kept');
               fr.register(t, 'token-dead', {});
             })();
             function check() {
               if (wr.deref() !== undefined) throw new Error('WeakRef target survived');
               if (wm.get(kept).v !== 42 || !ws.has(kept)) throw new Error('live key lost');
             }
             function checkLog() {
               if (log.sort().join() !== 'gone,token-dead') throw new Error('log ' + log);
             }",
        )
        .unwrap();
        // [[KeptAlive]] holds the WeakRef target until the checkpoint ends.
        e.microtask_checkpoint(100).unwrap();
        e.collect();
        e.call_global("check").unwrap();
        assert_eq!(e.pending_finalization_cleanups(), 1);
        e.run_finalization_cleanup().unwrap();
        e.microtask_checkpoint(100).unwrap();
        e.call_global("checkLog").unwrap();
        e.release_globals();
        assert_eq!(
            (e.live_objects(), e.live_roots(), e.charged_bytes()),
            (0, 0, 0)
        );
    }
}

/// CleanupFinalizationRegistry re-reads the cells on every round (WEAK_REFS.md): a
/// callback that unregisters another pending cell suppresses it, cells it registers are
/// kept, and a throwing callback leaves the other cells pending and other registries run.
#[test]
fn finalization_cleanup_is_reentrant() {
    for model in [Model::Stable, Model::Indirect] {
        let mut e = Engine::new(model, model).unwrap();
        e.execute(
            "var log = [], tokB = {};
             var fr = new FinalizationRegistry(function (h) {
               log.push(h);
               if (h === 'a') { fr.unregister(tokB); fr.register({}, 'new'); }
             });
             var bad = new FinalizationRegistry(function (h) {
               log.push(h);
               if (h === 'x') throw new Error('boom');
             });
             (function () {
               fr.register({}, 'a');
               fr.register({}, 'b', tokB);
               bad.register({}, 'x');
               bad.register({}, 'y');
             })();
             function logIs(s) {
               var got = log.slice().sort().join();
               if (got !== s) throw new Error('log ' + got + ' wanted ' + s);
             }
             function first() { logIs('a,x'); }
             function second() { logIs('a,new,x,y'); }",
        )
        .unwrap();
        e.microtask_checkpoint(100).unwrap();
        e.collect();
        assert_eq!(e.pending_finalization_cleanups(), 2);
        // `bad` throws on 'x'; `fr` still runs, and 'b' is gone with its token.
        assert!(e.run_finalization_cleanup().is_err());
        e.call_global("first").unwrap();
        // 'y' stays pending; 'new' (registered by the callback) dies at the next GC.
        e.collect();
        while e.pending_finalization_cleanups() != 0 {
            e.run_finalization_cleanup().unwrap();
        }
        e.call_global("second").unwrap();
        e.release_globals();
        assert_eq!(
            (e.live_objects(), e.live_roots(), e.charged_bytes()),
            (0, 0, 0)
        );
    }
}

/// Every route that marks a key drains its parked values (the CVE-2021-37975 bug class,
/// WEAK_REFS.md): a native seed, a key reached before its container and one reached
/// after it (the mark stack is LIFO, so both seed orders are run).
#[test]
fn ephemeron_key_marked_by_any_route_keeps_value() {
    for model in [Model::Stable, Model::Indirect] {
        for map_first in [false, true] {
            let mut r = Runtime::new(model, model).unwrap();
            let map = r.allocate(Domain::Js, 1, 0).unwrap();
            let holder = r.allocate(Domain::Js, 2, 0).unwrap();
            let mid = r.allocate(Domain::Js, 3, 0).unwrap();
            let key = r.allocate(Domain::Js, 4, 0).unwrap();
            let value = r.allocate(Domain::Js, 5, 0).unwrap();
            // The key is reachable only through a strong path from `holder`.
            r.link(holder, mid).unwrap();
            r.link(mid, key).unwrap();
            r.set_weak(map, vec![], vec![(key, value)]).unwrap();
            let seeds = if map_first {
                [map, holder]
            } else {
                [holder, map]
            };
            r.collect_joint(&seeds).unwrap();
            assert!(r.value(value).is_ok(), "value lost, map_first={map_first}");
            // Without the holder, the key and its value die together.
            r.collect_joint(&[map]).unwrap();
            assert_eq!(r.value(key), Err(Error::InvalidHandle));
            assert_eq!(r.value(value), Err(Error::InvalidHandle));
            r.collect_joint(&[]).unwrap();
            assert_eq!((r.live_objects(), r.charged_bytes()), (0, 0));
        }
    }
}

/// Key-indexed marking computes the same live set as the naive ephemeron fixpoint on
/// random heaps (strong links, several maps, ephemeron chains and cycles).
#[test]
fn ephemeron_marking_matches_naive_fixpoint() {
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for round in 0..40 {
        let n = 60;
        let links: Vec<(usize, usize)> = (0..n).map(|_| (next(n), next(n))).collect();
        let maps: Vec<usize> = (0..4).map(|_| next(n)).collect();
        let ephemerons: Vec<(usize, usize, usize)> = (0..40)
            .map(|_| (maps[next(maps.len())], next(n), next(n)))
            .collect();
        let roots: Vec<usize> = (0..3).map(|_| next(n)).collect();
        // Naive fixpoint: repeat until no ephemeron adds a value.
        let mut live = vec![false; n];
        let mut stack = roots.clone();
        loop {
            while let Some(x) = stack.pop() {
                if !live[x] {
                    live[x] = true;
                    stack.extend(links.iter().filter(|l| l.0 == x).map(|l| l.1));
                }
            }
            stack.extend(
                ephemerons
                    .iter()
                    .filter(|&&(m, k, v)| live[m] && live[k] && !live[v])
                    .map(|e| e.2),
            );
            if stack.is_empty() {
                break;
            }
        }
        let mut r = Runtime::new(Model::Stable, Model::Stable).unwrap();
        let ids: Vec<_> = (0..n)
            .map(|i| r.allocate(Domain::Js, i as i64, 0).unwrap())
            .collect();
        for &(a, b) in &links {
            r.link(ids[a], ids[b]).unwrap();
        }
        let mut per_map: Vec<(usize, Vec<_>)> = vec![];
        for &(m, k, v) in &ephemerons {
            match per_map.iter_mut().find(|p| p.0 == m) {
                Some(p) => p.1.push((ids[k], ids[v])),
                None => per_map.push((m, vec![(ids[k], ids[v])])),
            }
        }
        for (m, list) in per_map {
            r.set_weak(ids[m], vec![], list).unwrap();
        }
        let seeds: Vec<_> = roots.iter().map(|&x| ids[x]).collect();
        r.collect_joint(&seeds).unwrap();
        for i in 0..n {
            assert_eq!(r.value(ids[i]).is_ok(), live[i], "round {round} node {i}");
        }
    }
}
