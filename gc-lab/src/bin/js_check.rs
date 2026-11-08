//! One strict-script Test262 case per process; no source rewriting or helper stubs.
//! `--include=FILE` prepends an official harness include (as the reference runner does).
use gc_lab::{
    script::{Engine, ScriptError},
    Model,
};
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("js_check CASE.js [expected runtime error]");
    let remaining: Vec<_> = args.collect();
    let asynchronous = remaining.iter().any(|s| s == "--async");
    let mut includes = String::new();
    for file in remaining
        .iter()
        .filter_map(|s| s.strip_prefix("--include="))
    {
        includes.push_str(&std::fs::read_to_string(file).expect("include"));
        includes.push('\n');
    }
    let negative = remaining
        .into_iter()
        .find(|s| s != "--async" && !s.starts_with("--include="));
    let source = std::fs::read_to_string(path).expect("source");
    let mut e = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    e.enable_test_harness();
    let full = format!("\"use strict\";\n{includes}{source}");
    let mut result = e.execute(&full);
    if let (Err(ScriptError::Syntax(s)), Ok(_)) =
        (&result, std::env::var("JS_CHECK_SYNTAX_CONTEXT"))
    {
        // Debug aid for lexer/parser surveys: the offending character and its line.
        let at = s.offset.min(full.len());
        let start = full[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = full[at..].find('\n').map_or(full.len(), |i| at + i);
        let c = full[at..].chars().next().map_or(0, |c| c as u32);
        eprintln!("SYNTAX\t{}\tU+{c:04X}\t{}", s.message, &full[start..end]);
    }
    if result.is_ok() {
        result = e.microtask_checkpoint(10_000).map(|_| ());
    }
    let (done, error) = e.async_test_state();
    let harness_error = if result.is_err() {
        None
    } else if let Some(error) = error {
        Some(format!("$DONE: {error}"))
    } else if asynchronous && done != 1 {
        Some(format!("expected one $DONE, got {done}"))
    } else if let Some(error) = e.unhandled_test_failure() {
        Some(error)
    } else {
        let errors = e.take_microtask_errors();
        if errors.is_empty() {
            None
        } else {
            Some(format!("microtask errors: {errors:?}"))
        }
    };

    let assertions = e.assertion_count();
    e.release_globals();
    let clean = e.live_objects() == 0
        && e.metadata_objects() == 0
        && e.live_roots() == 0
        && e.charged_bytes() == 0;
    let (status, reason) = if !clean {
        ("failed", "GC cleanup failed".into())
    } else if let Some(error) = harness_error {
        ("failed", error)
    } else {
        match (result, negative) {
            (Err(ScriptError::Syntax(s)), _) => ("unsupported", format!("syntax: {}", s.message)),
            (Err(ScriptError::Unsupported(s)), _) => ("unsupported", s.into()),
            (Err(ScriptError::Thrown(ex)), Some(expected))
                if ex.name.as_deref() == Some(expected.as_str()) =>
            {
                ("passed", String::new())
            }
            (Ok(()), None) => ("passed", String::new()),
            (result, expected) => ("failed", format!("{result:?}; expected {expected:?}")),
        }
    };
    println!(
        "{{\"status\":{},\"reason\":{},\"assertions\":{assertions},\"cleanup\":{clean}}}",
        quote(status),
        quote(&reason)
    );
    std::process::exit(match status {
        "passed" => 0,
        "unsupported" => 2,
        _ => 1,
    });
}
