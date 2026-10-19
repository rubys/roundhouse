//! `App::sidekiq_redis`: the `url:` and `password:` of an app's
//! `Sidekiq.configure_client` block, read at ingest from
//! `config/initializers`. Other keys, and values in forms ingest doesn't
//! read, are warnings and are left to the environment fallbacks.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::app::{RedisSetting, SidekiqRedisConfig};
use roundhouse::diagnostic::{Diagnostic, DiagnosticKind};
use roundhouse::ingest::{ingest_app_from_tree, prism};

const SCHEMA: &str = "ActiveRecord::Schema.define do\nend\n";

fn ingest(init: &str) -> (SidekiqRedisConfig, Vec<String>) {
    let tree: HashMap<PathBuf, Vec<u8>> = [("db/schema.rb", SCHEMA), ("config/initializers/sidekiq.rb", init)]
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let (app, diags) = prism::scope(|| ingest_app_from_tree(tree));
    (app.expect("ingest").sidekiq_redis, sidekiq_warnings(diags))
}

fn sidekiq_warnings(diags: Vec<Diagnostic>) -> Vec<String> {
    diags
        .into_iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Sidekiq"))
        .map(|d| d.message)
        .collect()
}

fn env(name: &str) -> Option<RedisSetting> {
    Some(RedisSetting::Env(name.to_string()))
}

#[test]
fn nothing_configured_is_empty() {
    let (config, warnings) = ingest("Rails.application.config.x = 1\n");
    assert!(config.is_empty());
    assert!(warnings.is_empty(), "{warnings:#?}");
}

#[test]
fn url_and_password_in_each_form() {
    let (config, warnings) = ingest(
        "Sidekiq.configure_client do |config|\n  config.redis = { url: ENV[\"JOBS_REDIS_URL\"], password: ENV.fetch(\"JOBS_REDIS_PASSWORD\") }\nend\n",
    );
    assert!(warnings.is_empty(), "{warnings:#?}");
    assert_eq!(config.url, env("JOBS_REDIS_URL"));
    assert_eq!(
        config.password,
        Some(RedisSetting::EnvFetch { name: "JOBS_REDIS_PASSWORD".into(), default: None })
    );

    let (config, _) = ingest(
        "Sidekiq.configure_client do |c|\n  c.redis = { url: ENV.fetch(\"REDIS_URL\", \"redis://localhost:6379/1\"), password: \"hunter2\" }\nend\n",
    );
    assert_eq!(
        config.url,
        Some(RedisSetting::EnvFetch { name: "REDIS_URL".into(), default: Some("redis://localhost:6379/1".into()) })
    );
    assert_eq!(config.password, Some(RedisSetting::Literal("hunter2".into())));

    let (config, _) = ingest("Sidekiq.configure_client { |c| c.redis = { url: \"redis://cache:6379/2\" } }\n");
    assert_eq!(config.url, Some(RedisSetting::Literal("redis://cache:6379/2".into())));
    assert_eq!(config.password, None);
}

#[test]
fn other_keys_are_warned_about_and_ignored() {
    let (config, warnings) = ingest(
        "Sidekiq.configure_client do |config|\n  config.redis = { url: ENV[\"REDIS_URL\"], size: 5, ssl_params: { verify_mode: 0 } }\nend\n",
    );
    assert_eq!(config.url, env("REDIS_URL"));
    assert_eq!(warnings.len(), 2, "{warnings:#?}");
    assert!(warnings.iter().any(|w| w.contains("`size:`")), "{warnings:#?}");
    assert!(warnings.iter().any(|w| w.contains("`ssl_params:`")), "{warnings:#?}");
}

#[test]
fn an_unreadable_value_is_warned_about_and_left_to_the_fallback() {
    let (config, warnings) = ingest(
        "Sidekiq.configure_client do |config|\n  config.redis = { url: Rails.application.credentials.redis_url }\nend\n",
    );
    assert_eq!(config.url, None);
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].contains("credentials.redis_url"), "{warnings:#?}");
}

#[test]
fn a_hash_held_in_a_variable_is_warned_about() {
    let (config, warnings) = ingest(
        "redis = { url: ENV[\"REDIS_URL\"] }\nSidekiq.configure_client do |config|\n  config.redis = redis\nend\n",
    );
    assert!(config.is_empty());
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert!(warnings[0].contains("not a Hash literal"), "{warnings:#?}");
}

#[test]
fn configure_server_is_not_the_client() {
    let (config, warnings) = ingest(
        "Sidekiq.configure_server do |config|\n  config.redis = { url: ENV[\"SERVER_REDIS_URL\"] }\nend\n",
    );
    assert!(config.is_empty());
    assert!(warnings.is_empty(), "{warnings:#?}");
}
