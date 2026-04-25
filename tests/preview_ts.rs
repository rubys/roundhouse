//! Scratch: print emitted target output for our fixtures.
//! Run one of:
//!   cargo test --test preview_ts dump_tiny_blog_ts     -- --ignored --nocapture
//!   cargo test --test preview_ts dump_tiny_blog_elixir -- --ignored --nocapture
//!   cargo test --test preview_ts dump_real_blog_ts     -- --ignored --nocapture

use std::path::Path;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::{elixir, typescript};
use roundhouse::ingest::ingest_app;

fn analyzed(fixture: &str) -> roundhouse::App {
    let mut app = ingest_app(Path::new(fixture)).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
#[ignore]
fn dump_tiny_blog_ts() {
    for f in typescript::emit(&analyzed("fixtures/tiny-blog")) {
        println!("// ======= {} =======", f.path.display());
        println!("{}", f.content);
    }
}

#[test]
#[ignore]
fn dump_tiny_blog_elixir() {
    for f in elixir::emit(&analyzed("fixtures/tiny-blog")) {
        println!("# ======= {} =======", f.path.display());
        println!("{}", f.content);
    }
}

#[test]
#[ignore]
fn dump_real_blog_ts() {
    for f in typescript::emit(&analyzed("fixtures/real-blog")) {
        println!("// ======= {} =======", f.path.display());
        println!("{}", f.content);
    }
}

#[test]
#[ignore]
fn dump_real_blog_rust() {
    for f in roundhouse::emit::rust::emit(&analyzed("fixtures/real-blog")) {
        println!("// ======= {} =======", f.path.display());
        println!("{}", f.content);
    }
}

/// Dump the model TS output for `transpiled_blog` — used while
/// landing the post-lowering input shape end-to-end. Ignored because
/// it's a manual inspection helper, not a regression test.
#[test]
#[ignore]
fn dump_transpiled_blog_ts() {
    for f in typescript::emit(&analyzed("runtime/ruby/test/fixtures/transpiled_blog")) {
        if f.path.starts_with("app/models") {
            println!("// ======= {} =======", f.path.display());
            println!("{}", f.content);
        }
    }
}
