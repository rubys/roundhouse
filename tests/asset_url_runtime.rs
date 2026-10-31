//! `ActionView::ViewHelpers.asset_url`, which a bare `asset_url(...)` in
//! a non-view class lowers to. a large app's `Icons::DefaultIcon
//! .url` calls it with `skip_pipeline: true` on every app-install path;
//! the runtime had `asset_path` and `image_url` but no `asset_url`
//! (`undefined method 'asset_url' for module ActionView::ViewHelpers`).
//!
//! With no request or `config.asset_host` in reach, Rails answers the
//! same path `asset_path` does.
//!
//! Skips when `ruby` with the sqlite3 gem is not on the machine.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

#[test]
fn asset_url_answers_the_asset_path() {
    if !Command::new("ruby").args(["-rsqlite3", "-e", "1"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with the sqlite3 gem not available");
        return;
    }
    let out = emit_and_run::real_blog().run_ruby(
        r#"
puts ActionView::ViewHelpers.asset_url("/assets/default-app.png", skip_pipeline: true)
puts ActionView::ViewHelpers.asset_url("logo.png")
puts ActionView::ViewHelpers.asset_url("https://cdn.example/x.png")
"#,
    );
    out.assert_passes();
    assert_eq!(
        out.stdout.lines().collect::<Vec<_>>(),
        ["/assets/default-app.png", "/assets/logo.png", "https://cdn.example/x.png"],
        "{}",
        out.stdout
    );
}
