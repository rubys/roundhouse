use std::path::Path;
use roundhouse::analyze::Analyzer;
use roundhouse::emit::{crystal, elixir, go, python, rust, typescript};
use roundhouse::ingest::ingest_app;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let target = if let Some(i) = args.iter().position(|a| a == "--target") {
        args.remove(i);
        if i < args.len() { args.remove(i) } else { "typescript".into() }
    } else {
        "typescript".into()
    };
    let library_mode = args.iter().position(|a| a == "--library").map(|i| { args.remove(i); true }).unwrap_or(false);
    let fixture = args.first().cloned().unwrap_or_else(|| "fixtures/real-blog".into());
    let mut app = ingest_app(Path::new(&fixture)).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let (files, out_dir) = match target.as_str() {
        "typescript" | "ts" => {
            let emitted = if library_mode {
                typescript::emit_library(&app)
            } else {
                typescript::emit(&app)
            };
            let dir = if library_mode { "/tmp/rh-ts-lib" } else { "/tmp/rh-ts-pass2" };
            (emitted, dir)
        }
        "crystal" | "cr" => (crystal::emit(&app), "/tmp/rh-cr-pass2"),
        "rust" | "rs" => (rust::emit(&app), "/tmp/rh-rs-pass2"),
        "python" | "py" => (python::emit(&app), "/tmp/rh-py-pass2"),
        "elixir" | "ex" => (elixir::emit(&app), "/tmp/rh-ex-pass2"),
        "go" => (go::emit(&app), "/tmp/rh-go-pass2"),
        other => panic!("unknown target: {other}"),
    };
    let out = Path::new(out_dir);
    if out.exists() { std::fs::remove_dir_all(out).ok(); }
    std::fs::create_dir_all(out).ok();
    for f in &files {
        let path = out.join(&f.path);
        if let Some(p) = path.parent() { std::fs::create_dir_all(p).ok(); }
        std::fs::write(&path, &f.content).expect("write");
    }
    println!("emitted {} files to {}", files.len(), out_dir);
}
