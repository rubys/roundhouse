//! Scratch: print the emitted TypeScript for tiny-blog.
//! Run with `cargo test --test preview_ts -- --ignored --nocapture`.

use std::path::Path;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::typescript;
use roundhouse::ingest::ingest_app;

#[test]
#[ignore]
fn dump_tiny_blog_ts() {
    let mut app = ingest_app(Path::new("fixtures/tiny-blog")).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    for f in typescript::emit(&app) {
        println!("// ======= {} =======", f.path.display());
        println!("{}", f.content);
    }
}
