//! `config.session_options[:key]` → `session_cookie_key`
//! (`lower::apply_session_options_lowering`), and the ingest lift that
//! gives it a value (`config.session_store :cookie_store, key: "..."`
//! from `config/initializers/session_store.rb`).
//!
//! Rails keeps the session cookie name in a heterogeneous options bag
//! (`expire_after` is a Duration, `httponly` a bool, `same_site` a
//! Symbol), so a modelled `session_options` would hand back untyped —
//! which is what AOT-refused lobsters' `key ==
//! Rails.application.config.session_options[:key]` as an UNKNOWN-vs-String
//! equality. The name is knowable at transpile time, so the read grounds
//! to a typed String accessor and the bag is never built.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_session_options_lowering;

fn tree(files: Vec<(&str, &str)>) -> roundhouse::App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

fn controller_body_debug(app: &roundhouse::App) -> String {
    let controller = app
        .controllers
        .iter()
        .find(|c| c.name.0.as_str() == "HomeController")
        .expect("HomeController ingested");
    controller
        .body
        .iter()
        .filter_map(|item| match item {
            roundhouse::dialect::ControllerBodyItem::Action { action, .. } => {
                Some(format!("{:?}", action.body))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const CONTROLLER: &str = r#"class HomeController < ApplicationController
  def index
    key = Rails.application.config.session_options[:key]
    request.session_options[:skip] = true
    @same = Rails.application.config.session_options[:same_site]
  end
end
"#;

#[test]
fn app_config_session_key_grounds_to_the_typed_accessor() {
    let mut app = tree(vec![("app/controllers/home_controller.rb", CONTROLLER)]);
    let before = controller_body_debug(&app);
    apply_session_options_lowering(&mut app);
    let after = controller_body_debug(&app);

    // The `:key` read is now a zero-arg send on the same receiver.
    assert!(!before.contains("session_cookie_key"), "precondition");
    assert!(
        after.contains("session_cookie_key"),
        "expected session_cookie_key accessor, got:\n{after}"
    );
    // Exactly one `session_options` hop disappears — the `:key` read.
    // The other two in CONTROLLER (the per-request `:skip` write and the
    // unmodelled `:same_site` read) are untouched, so this pins that the
    // pass claims the one site and no more.
    let n_before = before.matches("session_options").count();
    let n_after = after.matches("session_options").count();
    assert_eq!(
        n_before - 1,
        n_after,
        "expected exactly one session_options hop to ground \
         ({n_before} -> {n_after}):\n{after}"
    );
}

#[test]
fn per_request_session_options_write_is_left_alone() {
    let mut app = tree(vec![("app/controllers/home_controller.rb", CONTROLLER)]);
    apply_session_options_lowering(&mut app);
    let body = controller_body_debug(&app);
    // `request.session_options[:skip] = true` is rack's WRITABLE
    // per-request bag — a different surface from the application config,
    // and a write besides. It must survive untouched.
    assert!(
        body.contains("skip"),
        "per-request session_options write should survive:\n{body}"
    );
}

#[test]
fn only_the_key_option_grounds_other_options_stay_verbatim() {
    let mut app = tree(vec![("app/controllers/home_controller.rb", CONTROLLER)]);
    apply_session_options_lowering(&mut app);
    let body = controller_body_debug(&app);
    // `:same_site` has no typed accessor and no transpile-time value we
    // model; it keeps refusing loudly rather than being silently stubbed.
    assert!(
        body.contains("same_site"),
        "non-:key options should stay verbatim:\n{body}"
    );
}

/// The declared key reaches `Rails::Application` as a `session_cookie_key`
/// def, so both the dispatch and app code read one value.
fn lifted_key(initializer: &str) -> Option<String> {
    let app = tree(vec![
        ("app/controllers/home_controller.rb", CONTROLLER),
        (
            "config/application.rb",
            "module Demo\n  class Application < Rails::Application\n    def name\n      \"Demo\"\n    end\n  end\nend\n",
        ),
        ("config/initializers/session_store.rb", initializer),
    ]);
    let rails_app = app.rails_application.as_ref()?;
    let m = rails_app
        .methods
        .iter()
        .find(|m| m.name.as_str() == "session_cookie_key")?;
    Some(format!("{:?}", m.body))
}

#[test]
fn session_store_key_lifts_from_the_initializer() {
    // Wrapped form with double quotes — current lobsters.
    let body = lifted_key(
        "Demo::Application.config.session_store :cookie_store,\n  key: \"lobster_trap\",\n  expire_after: 1.month\n",
    )
    .expect("session_cookie_key synthesized");
    assert!(body.contains("lobster_trap"), "got: {body}");
}

#[test]
fn single_quotes_and_deep_indentation_lift_too() {
    // The ruby-bench lobsters snapshot's spelling: single quotes, the
    // options aligned far right under the call.
    let body = lifted_key(
        "Demo::Application.config.session_store :cookie_store,\n                                           key: 'lobster_trap',\n                                           expire_after: 1.month\n",
    )
    .expect("session_cookie_key synthesized");
    assert!(body.contains("lobster_trap"), "got: {body}");
}

#[test]
fn one_line_form_lifts() {
    let body = lifted_key(
        "Demo::Application.config.session_store :cookie_store, key: \"jar\"\n",
    )
    .expect("session_cookie_key synthesized");
    assert!(body.contains("jar"), "got: {body}");
}

#[test]
fn commented_out_declaration_does_not_lift() {
    // No app override — the framework default in runtime/ruby/rails.rb
    // stands, which is what a Rails app with no session_store gets.
    assert!(
        lifted_key("# Demo::Application.config.session_store :cookie_store, key: \"nope\"\n")
            .is_none(),
        "a commented-out declaration must not lift"
    );
}

#[test]
fn a_lookalike_option_label_does_not_lift() {
    // `secret_key:` must not be mistaken for `key:`.
    assert!(
        lifted_key(
            "Demo::Application.config.session_store :cookie_store, secret_key: \"nope\"\n"
        )
        .is_none(),
        "only a standalone `key:` label lifts"
    );
}
