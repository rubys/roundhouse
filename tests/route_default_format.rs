//! A helper's requested format that matches the route's `defaults:`
//! format drops the extension, as Rails does. The expectations were
//! produced by railties/actionpack 8.1.4 drawing
//! `route_default_format_routes.rb` into a `RouteSet` and calling its
//! `url_helpers` from `route_default_format_model.rb`, not derived from
//! the implementation. CI installs neither gem, so the oracle is not
//! rerun here.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const RAILS_8_1_4_PATHS: &str = r#"["/manifest", "/manifest", "/manifest", "/manifest.xml", "/plain.json", "/string_manifest", "/string_manifest.xml", "/scoped/7", "/scoped/7.xml", "/scoped/7?page=2", "/scoped/7.xml?page=2"]"#;

#[test]
fn route_default_formats_match_rails_in_emitted_helpers() {
    let routes = include_str!("route_default_format_routes.rb");
    let model = include_str!("route_default_format_model.rb");
    emit_and_run::empty_app()
        .write("config/routes.rb", &format!("Rails.application.routes.draw do\n{routes}end\n"))
        .write("app/services/format_route_probe.rb", model)
        .run_ruby(&format!("expected = {RAILS_8_1_4_PATHS}\nactual = FormatRouteProbe.paths\nraise actual.inspect unless actual == expected\nputs \"Route format defaults match Rails\"\n"))
        .assert_passes();
}
