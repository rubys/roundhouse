//! `ActionController::Base#view_context` answers an `ActionView::Base` whose
//! `asset_path` / `asset_url` follow Rails' AssetUrlHelper for a context built
//! outside a request.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// An app's asset helper object asks
/// `ApplicationController.new.view_context` for `asset_url` / `asset_path`
/// outside a request, with an options hash (`host:`, a CDN override):
/// `undefined method 'view_context' for an instance of ApplicationController`.
/// The answers are Rails' AssetUrlHelper's for a context without a request.
#[test]
fn a_controllers_view_context_answers_asset_url_and_path() {
    let out = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(
        r#"
view = ApplicationController.new.view_context
puts view.asset_url("logo.png", {})
puts view.asset_url("logo.png?v=2#top")
puts view.asset_path("logo.png", host: "cdn.example.com/cdn")
puts view.asset_url("logo.png", host: "https://cdn.example.com")
puts view.asset_url("https://other.example/x.png", host: "cdn.example.com")
puts view.asset_url("/already/rooted.png")
puts view.asset_url("raw.png", skip_pipeline: true)
puts view.asset_url("").inspect
begin
  view.asset_url(nil)
rescue ArgumentError => e
  puts e.message
end
"#,
    );
    out.assert_passes();
    assert_eq!(
        out.stdout.lines().collect::<Vec<_>>(),
        [
            "/assets/logo.png",
            "/assets/logo.png?v=2#top",
            "//cdn.example.com/cdn/assets/logo.png",
            "https://cdn.example.com/assets/logo.png",
            "https://other.example/x.png",
            "/already/rooted.png",
            "/raw.png",
            "\"\"",
            "nil is not a valid asset source"
        ],
        "{}",
        out.stdout
    );
}
