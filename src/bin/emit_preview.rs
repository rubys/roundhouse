use std::path::Path;
use roundhouse::analyze::Analyzer;
use roundhouse::emit::typescript;
use roundhouse::ingest::ingest_app;

fn main() {
    let fixture = std::env::args().nth(1).unwrap_or_else(|| "fixtures/real-blog".into());
    let mut app = ingest_app(Path::new(&fixture)).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = typescript::emit(&app);
    let out = Path::new("/tmp/rh-ts-pass2");
    if out.exists() { std::fs::remove_dir_all(out).ok(); }
    std::fs::create_dir_all(out).ok();
    for f in &files {
        let path = out.join(&f.path);
        if let Some(p) = path.parent() { std::fs::create_dir_all(p).ok(); }
        std::fs::write(&path, &f.content).expect("write");
    }
}
