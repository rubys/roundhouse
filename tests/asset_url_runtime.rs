//! `ActionView::ViewHelpers.asset_url`, which a bare `asset_url(...)` in
//! a non-view class lowers to. a large app's `Icons::DefaultIcon
//! .url` calls it with `skip_pipeline: true` on every app-install path;
//! the runtime had `asset_path` and `image_url` but no `asset_url`
//! (`undefined method 'asset_url' for module ActionView::ViewHelpers`).
//!
//! With no request or `config.asset_host` in reach, Rails answers the
//! same path `asset_path` does; `data:` and `cid:` sources pass through.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn asset_url_answers_the_asset_path() {
    let out = emit_and_run::real_blog().run_ruby(
        r#"
puts ActionView::ViewHelpers.asset_url("/assets/default-app.png", skip_pipeline: true)
puts ActionView::ViewHelpers.asset_url("logo.png")
puts ActionView::ViewHelpers.asset_url("https://cdn.example/x.png")
puts ActionView::ViewHelpers.asset_url("data:image/png;base64,abc")
puts ActionView::ViewHelpers.asset_url("DATA:image/png;base64,abc")
puts ActionView::ViewHelpers.asset_url("Cid:logo")
"#,
    );
    out.assert_passes();
    assert_eq!(
        out.stdout.lines().collect::<Vec<_>>(),
        ["/assets/default-app.png", "/assets/logo.png", "https://cdn.example/x.png", "data:image/png;base64,abc", "DATA:image/png;base64,abc", "Cid:logo"],
        "{}",
        out.stdout
    );
}
