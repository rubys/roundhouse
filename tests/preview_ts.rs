//! Scratch: print emitted target output for tiny-blog.
//! Run one of:
//!   cargo test --test preview_ts dump_tiny_blog_ts     -- --ignored --nocapture
//!   cargo test --test preview_ts dump_tiny_blog_elixir -- --ignored --nocapture

use std::path::Path;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::{elixir, typescript};
use roundhouse::ingest::ingest_app;

fn analyzed() -> roundhouse::App {
    let mut app = ingest_app(Path::new("fixtures/tiny-blog")).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
#[ignore]
fn dump_tiny_blog_ts() {
    for f in typescript::emit(&analyzed()) {
        println!("// ======= {} =======", f.path.display());
        println!("{}", f.content);
    }
}

#[test]
#[ignore]
fn dump_tiny_blog_elixir() {
    for f in elixir::emit(&analyzed()) {
        println!("# ======= {} =======", f.path.display());
        println!("{}", f.content);
    }
}
