//! Experimental out-of-process interpreter. Servo remains a separate browser.
use gc_lab::{
    script::{
        wire::{encode, LineAdapter},
        Engine, GcPolicy,
    },
    Model,
};
use std::io::{self, Write};
fn emit(line: &str) {
    println!("{line}");
    io::stdout().flush().unwrap();
}
fn released(engine: &mut Engine) {
    engine.release_globals();
    emit(&format!(
        "RELEASED\t{}\t{}\t{}\t{}\t{}",
        engine.live_objects(),
        engine.metadata_objects(),
        engine.live_roots(),
        engine.remote_node_count(),
        encode(engine.host_error().unwrap_or(""))
    ));
}
fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: servo_bridge SOURCE.js");
        std::process::exit(2);
    };
    let source = std::fs::read_to_string(path).expect("source file");
    let mut engine = Engine::new(Model::Indirect, Model::Indirect).unwrap();
    engine.set_gc_policy(GcPolicy::Threshold);
    engine.set_dom_adapter(Box::new(LineAdapter)).unwrap();
    if let Err(error) = engine.execute(&source) {
        released(&mut engine);
        emit(&format!("FAIL\t{}", encode(&format!("{error:?}"))));
        std::process::exit(1);
    }
    emit("READY");
    loop {
        let mut line = String::new();
        if io::stdin().read_line(&mut line).unwrap() == 0 {
            released(&mut engine);
            break;
        }
        let command = line.trim_end_matches(['\n', '\r']);
        if command == "RELEASE" {
            released(&mut engine);
        } else if command == "QUIT" {
            released(&mut engine);
            emit("BYE");
            break;
        } else if let Some(node) = command
            .strip_prefix("EVENT\t")
            .and_then(|v| v.parse::<u64>().ok())
        {
            if let Err(error) = engine.dispatch_remote_click(node) {
                released(&mut engine);
                emit(&format!("FAIL\t{}", encode(&format!("{error:?}"))));
                std::process::exit(1);
            }
            engine.collect();
            emit("EVENT_DONE");
        } else {
            released(&mut engine);
            emit(&format!("FAIL\t{}", encode("invalid control command")));
            std::process::exit(1);
        }
    }
    if engine.host_error().is_some() {
        std::process::exit(1);
    }
}
