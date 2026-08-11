//! Rails' `config.x` custom-config namespace, both halves.
//!
//! `config.x.vapid.public_key = …` in an initializer IS the definition —
//! the same premise the plain `config.<key>` lift already rests on — so
//! ingest lifts it to a reader on the `Rails::Application` reopen and
//! `lower::config_reader` rewrites the reads to call it. Both halves
//! FLATTEN the path to one name (`x_vapid_public_key`), which is what
//! lets them meet without modelling `x` as nested OrderedOptions.
//!
//! Campfire's layout reads `Rails.configuration.x.vapid.public_key` on
//! every page, so this sits between the emit and any rendered page.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::App;

fn app_with(initializer: &str) -> App {
    // The whole config lift is gated on `config/application.rb` existing
    // — that file is where the `Rails::Application` reopen the readers
    // land on comes from.
    let tree = vec![
        (
            std::path::PathBuf::from("config/application.rb"),
            b"module Demo\n  class Application < Rails::Application\n  end\nend\n".to_vec(),
        ),
        (
            std::path::PathBuf::from("config/initializers/custom.rb"),
            initializer.as_bytes().to_vec(),
        ),
        (
            std::path::PathBuf::from("app/models/thing.rb"),
            b"class Thing < ApplicationRecord\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

fn lifted_readers(app: &App) -> Vec<String> {
    match &app.rails_application {
        Some(lc) => lc.methods.iter().map(|m| m.name.as_str().to_string()).collect(),
        None => Vec::new(),
    }
}

#[test]
fn x_namespace_assignment_lifts_to_a_flattened_reader() {
    let app = app_with(
        r#"
Rails.application.configure do
  config.x.vapid.public_key = "abc"
end
"#,
    );
    assert!(
        lifted_readers(&app).contains(&"x_vapid_public_key".to_string()),
        "expected a flattened reader: {:?}",
        lifted_readers(&app)
    );
}

#[test]
fn configure_block_is_descended_into() {
    // `Rails.application.configure do … end` is the form Rails' own
    // generator writes; before this the walk descended into class and
    // module bodies only, so every line inside it was invisible.
    let app = app_with(
        r#"
Rails.application.configure do
  config.app_version = "1.2.3"
end
"#,
    );
    assert!(
        lifted_readers(&app).contains(&"app_version".to_string()),
        "a plain key inside `configure do` must lift too: {:?}",
        lifted_readers(&app)
    );
}

#[test]
fn single_level_x_key_lifts() {
    let app = app_with("Rails.application.config.x.web_push_pool = 1\n");
    assert!(
        lifted_readers(&app).contains(&"x_web_push_pool".to_string()),
        "{:?}",
        lifted_readers(&app)
    );
}

#[test]
fn framework_subsection_is_not_lifted() {
    // `config.action_mailer.delivery_method` is FRAMEWORK config, not the
    // app's own namespace. Lifting it would invent a reader for a value
    // roundhouse does not define, which is exactly what the "fail
    // visibly" rule exists to prevent.
    let app = app_with("Rails.application.configure do\n  config.action_mailer.delivery_method = :test\nend\n");
    let readers = lifted_readers(&app);
    assert!(
        !readers.iter().any(|r| r.contains("action_mailer")),
        "framework subsections must stay unlifted: {readers:?}"
    );
}

#[test]
fn plain_config_key_still_lifts_unflattened() {
    // The pre-existing shape, kept honest: no `x` prefix, no flattening.
    let app = app_with("Rails.application.config.app_version = \"9\"\n");
    assert!(
        lifted_readers(&app).contains(&"app_version".to_string()),
        "{:?}",
        lifted_readers(&app)
    );
}
