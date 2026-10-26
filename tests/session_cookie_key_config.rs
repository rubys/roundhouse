use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> Result<roundhouse::App, roundhouse::ingest::IngestError> {
    ingest_app_from_tree(
        files
            .iter()
            .map(|(path, body)| (std::path::PathBuf::from(path), body.as_bytes().to_vec()))
            .collect(),
    )
}

#[test]
fn named_applications_derive_keys_like_rails() {
    for (source, expected) in [
        (
            "module CwtRails\nclass Application < Rails::Application\nend\nend",
            "_cwt_rails_session",
        ),
        (
            "module Outer\nmodule HTMLApp\nclass Application < Rails::Application\nend\nend\nend",
            "_outer_html_app_session",
        ),
        (
            "module Outer::HTMLApp\nclass Application < Rails::Application\nend\nend",
            "_outer_html_app_session",
        ),
        (
            "class NamedApp < Rails::Application\nend",
            "_named_app_session",
        ),
        (
            "module MyApplication\nclass Application < Rails::Application\nend\nend",
            "_my_application_session",
        ),
    ] {
        let app = tree(&[("config/application.rb", source)]).unwrap();
        let application = app.rails_application.unwrap();
        let method = application
            .methods
            .iter()
            .find(|m| m.name.as_str() == "session_cookie_key")
            .unwrap();
        let ruby = roundhouse::emit::ruby::emit_method(method);
        assert!(ruby.contains(expected), "{ruby}");
    }
}

#[test]
fn nonliteral_keys_and_session_store_calls_in_loops_are_refused() {
    for setting in [
        "config.session_store :cookie_store, key: ENV[\"COOKIE_KEY\"]",
        "config.session_store :cookie_store, key: \"_demo_#{ENV[\"COOKIE_KEY\"]}\"",
        "config.session_store :cookie_store, key: \"_demo_#{Rails.root}\"",
        "[1].each { config.session_store :cookie_store, key: \"bad\" }",
    ] {
        let source =
            format!("module Demo\nclass Application < Rails::Application\n{setting}\nend\nend");
        assert!(
            tree(&[("config/application.rb", &source)]).is_err(),
            "{setting}"
        );
    }
}

/// Interpolating `Rails.env` is the one non-literal part a key may have:
/// it is replayed so the compiled app reads its own environment.
#[test]
fn a_key_interpolating_rails_env_is_replayed_against_the_runtime_env() {
    let source = "module Demo\nclass Application < Rails::Application\nend\nend";
    let app = tree(&[
        ("config/application.rb", source),
        (
            "config/initializers/session_store.rb",
            "Rails.application.config.session_store :cookie_store, key: \"_demo_#{Rails.env}\"\n",
        ),
    ])
    .unwrap();
    let application = app.rails_application.unwrap();
    let method = application.methods.iter().find(|m| m.name.as_str() == "session_cookie_key").unwrap();
    let ruby = roundhouse::emit::ruby::emit_method(method);
    assert!(ruby.contains("\"_demo_#{Rails.env.to_s}\""), "{ruby}");
}

/// A literal key that contains `#{` (single-quoted in the source) stays
/// literal in the replay.
#[test]
fn a_literal_key_with_a_hash_brace_stays_literal() {
    let source = "module Demo\nclass Application < Rails::Application\nconfig.session_store :cookie_store, key: '_demo_#{x}'\nend\nend";
    let app = tree(&[("config/application.rb", source)]).unwrap();
    let application = app.rails_application.unwrap();
    let method = application.methods.iter().find(|m| m.name.as_str() == "session_cookie_key").unwrap();
    let ruby = roundhouse::emit::ruby::emit_method(method);
    assert!(ruby.contains(r#""_demo_\#{x}""#), "{ruby}");
}

/// `GlobalID.app` starts from the same railtie name as the session key,
/// dasherized as globalid's railtie does, so a gid Rails minted for a
/// multi-word or nested app name is one this runtime accepts.
#[test]
fn global_id_app_is_the_dasherized_railtie_name() {
    for (source, expected) in [
        ("module Campfire\nclass Application < Rails::Application\nend\nend", "\"campfire\""),
        ("module CwtRails\nclass Application < Rails::Application\nend\nend", "\"cwt-rails\""),
        (
            "module Outer\nmodule HTMLApp\nclass Application < Rails::Application\nend\nend\nend",
            "\"outer-html-app\"",
        ),
        (
            "module Outer::HTMLApp\nclass Application < Rails::Application\nend\nend",
            "\"outer-html-app\"",
        ),
        ("class NamedApp < Rails::Application\nend", "\"named-app\""),
        // globalid removes every `_application`, the session key only the last.
        ("module MyApplication\nclass Application < Rails::Application\nend\nend", "\"my\""),
    ] {
        let app = tree(&[("config/application.rb", source)]).unwrap();
        let application = app.rails_application.unwrap();
        let method = application
            .methods
            .iter()
            .find(|m| m.name.as_str() == "global_id_app")
            .unwrap();
        let ruby = roundhouse::emit::ruby::emit_method(method);
        assert!(ruby.contains(expected), "{ruby}");
    }
}
