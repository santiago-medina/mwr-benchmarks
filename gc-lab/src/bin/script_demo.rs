use gc_lab::{script::Engine, Model};
fn quote(value: &str) -> String {
    let mut result = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if c.is_control() => result.push_str(&format!("\\u{:04x}", c as u32)),
            c => result.push(c),
        }
    }
    result.push('"');
    result
}
fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: script_demo SOURCE.js [binding]");
        std::process::exit(2);
    };
    let binding = args.next().unwrap_or_else(|| "button".into());
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let mut engine = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    let result = engine.execute(&source).and_then(|_| {
        engine
            .text_content(&binding)
            .or_else(|_| engine.binding_string(&binding))
    });
    let before = engine.live_objects();
    engine.release_globals();
    let after = engine.live_objects();
    let metadata = engine.metadata_objects();
    let roots = engine.live_roots();
    match result {
        Ok(value) => println!("{{\"ok\":true,\"value\":{},\"live_before_release\":{before},\"live_after_release\":{after},\"metadata_after_release\":{metadata},\"roots_after_release\":{roots}}}", quote(&value)),
        Err(error) => {
            println!("{{\"ok\":false,\"error\":{},\"live_after_release\":{after},\"metadata_after_release\":{metadata},\"roots_after_release\":{roots}}}", quote(&format!("{error:?}")));
            std::process::exit(1);
        }
    }
}
