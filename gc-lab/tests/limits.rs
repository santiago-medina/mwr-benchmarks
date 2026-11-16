//! Interpreter guards (MOZJS_FACADE.md "Interpreter guards"): harness
//! defaults versus a native stack budget for embeddings.
use gc_lab::{
    script::{Engine, GcPolicy, Limits, ScriptError},
    Model,
};

const RECURSE: &str = "function f(n) { return n === 0 ? 0 : 1 + f(n - 1); }";

fn on_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn harness_defaults_keep_fixed_depths() {
    on_big_stack(|| {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        assert_eq!(e.limits(), Limits::HARNESS);
        let r = e.execute(&format!("{RECURSE} var r = f(40);"));
        assert!(matches!(r, Err(ScriptError::Runtime(_))), "{r:?}");
    });
}

#[test]
fn native_stack_budget_allows_deep_recursion_and_throws_range_error() {
    let report = on_big_stack(|| {
        let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
        e.set_gc_policy(GcPolicy::Threshold);
        e.set_step_limit(usize::MAX);
        e.set_limits(Limits::native_stack(192 << 20));
        e.execute(&format!("{RECURSE} var r = String(f(200));"))
            .unwrap();
        assert_eq!(e.binding_string("r").unwrap(), "200");
        let per_call = e.take_stack_high_water() / 200;
        e.execute(
            "function g() { return g() + 1; }
             var r = 'none';
             try { g(); } catch (x) { r = x instanceof RangeError ? x.message : 'other'; }
             try { g(); } catch (x) { r += '|' + (x instanceof RangeError); }",
        )
        .unwrap();
        (e.binding_string("r").unwrap(), per_call)
    });
    assert_eq!(report.0, "Maximum call stack size exceeded|true");
    eprintln!("native stack per JS call: {} bytes", report.1);
}
