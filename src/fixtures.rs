//! The generated Rails fixtures the test suites run against.
//!
//! `fixtures/real-blog` and `fixtures/store` are `.gitignore`d: `bin/rh
//! fixture` and `scripts/create-store` generate them from `rails new`
//! plus the scaffold / the Rails Guides walkthrough (CI does the same
//! once per run and fans the result out as an artifact). A fresh clone
//! has neither, and before [`crate::ingest::ingest_app`] refused a
//! missing root the tests that read them died on the *next* thing
//! they touched — `model Article not in real-blog`, `no emitted file
//! ending in articles_controller.rb; got: []` — with nothing naming the
//! fixture or the command. These accessors name both, once, at the
//! first touch.

use std::path::Path;

/// `fixtures/real-blog` — the Rails 8 scaffold blog (articles,
/// comments, Turbo Stream broadcasts). Panics with the generating
/// command when the directory is absent.
pub fn real_blog() -> &'static Path {
    present("fixtures/real-blog", "bin/rh fixture   # ~60s; needs Ruby and `gem install rails`")
}

/// `fixtures/store` — the Rails Guides store, built by following the
/// guide. Panics with the generating command when the directory is
/// absent.
pub fn store() -> &'static Path {
    present("fixtures/store", "(cd fixtures && ../scripts/create-store store)")
}

fn present(rel: &'static str, command: &str) -> &'static Path {
    let path = Path::new(rel);
    assert!(
        path.is_dir(),
        "{rel} is absent — it is generated, not checked in. From the repository root run:\n\n    {command}\n\n(see DEVELOPMENT.md, \"Fixtures\")",
    );
    path
}
