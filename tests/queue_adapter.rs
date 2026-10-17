//! `App::queue_adapter`: the ActiveJob adapter an app configures, read
//! at ingest from `config/application.rb`, `config/environments/
//! production.rb` and `config/initializers`, the last setting winning.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::diagnostic::{Diagnostic, DiagnosticKind};
use roundhouse::ingest::{ingest_app_from_tree, prism};
use roundhouse::App;

const SCHEMA: &str = "ActiveRecord::Schema.define do\nend\n";

fn ingest(files: &[(&str, &str)]) -> (App, Vec<Diagnostic>) {
    let mut all: Vec<(&str, &str)> = vec![("db/schema.rb", SCHEMA)];
    all.extend_from_slice(files);
    let tree: HashMap<PathBuf, Vec<u8>> =
        all.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let (app, diags) = prism::scope(|| ingest_app_from_tree(tree));
    (app.expect("ingest"), diags)
}

fn application(body: &str) -> String {
    format!(
        "require_relative \"boot\"\nrequire \"rails/all\"\n\nmodule Depot\n  class Application < Rails::Application\n    config.load_defaults 8.1\n{body}  end\nend\n"
    )
}

fn production(body: &str) -> String {
    format!("Rails.application.configure do\n{body}end\n")
}

fn adapter(files: &[(&str, &str)]) -> Option<String> {
    ingest(files).0.queue_adapter
}

#[test]
fn nothing_set_is_none() {
    assert_eq!(adapter(&[("config/application.rb", &application(""))]), None);
}

#[test]
fn a_symbol_and_a_string_both_name_the_adapter() {
    let sym = application("    config.active_job.queue_adapter = :sidekiq\n");
    let string = application("    config.active_job.queue_adapter = \"sidekiq\"\n");
    let single = application("    config.active_job.queue_adapter = 'sidekiq' # Redis\n");
    assert_eq!(adapter(&[("config/application.rb", &sym)]).as_deref(), Some("sidekiq"));
    assert_eq!(adapter(&[("config/application.rb", &string)]).as_deref(), Some("sidekiq"));
    assert_eq!(adapter(&[("config/application.rb", &single)]).as_deref(), Some("sidekiq"));
}

#[test]
fn a_comment_needs_no_space_before_it() {
    let sym = application("    config.active_job.queue_adapter = :sidekiq# Redis\n");
    let string = application("    config.active_job.queue_adapter = \"sidekiq\"# Redis\n");
    assert_eq!(adapter(&[("config/application.rb", &sym)]).as_deref(), Some("sidekiq"));
    assert_eq!(adapter(&[("config/application.rb", &string)]).as_deref(), Some("sidekiq"));
}

#[test]
fn a_hash_inside_the_quotes_is_not_a_comment() {
    let app = application("    config.active_job.queue_adapter = \"side#kiq\"\n");
    assert_eq!(adapter(&[("config/application.rb", &app)]), None);
}

#[test]
fn a_comparison_does_not_set_it() {
    let app = application(
        "    config.active_job.queue_adapter = :sidekiq\n    puts 1 if config.active_job.queue_adapter == :async\n    config.active_job.queue_adapter == :async\n    config.active_job.queue_adapter =~ /async/\n    config.active_job.queue_adapter => observed_adapter\n",
    );
    assert_eq!(adapter(&[("config/application.rb", &app)]).as_deref(), Some("sidekiq"));
}

#[test]
fn a_commented_out_line_is_ignored() {
    let app = application("    # config.active_job.queue_adapter = :sidekiq\n");
    assert_eq!(adapter(&[("config/application.rb", &app)]), None);
}

#[test]
fn production_rb_overrides_application_rb() {
    let app = application("    config.active_job.queue_adapter = :async\n");
    let prod = production("  config.active_job.queue_adapter = :sidekiq\n");
    assert_eq!(
        adapter(&[("config/application.rb", &app), ("config/environments/production.rb", &prod)])
            .as_deref(),
        Some("sidekiq")
    );
}

#[test]
fn an_initializer_overrides_both() {
    let app = application("    config.active_job.queue_adapter = :sidekiq\n");
    let prod = production("  config.active_job.queue_adapter = :solid_queue\n");
    let init = "Rails.application.config.active_job.queue_adapter = :sidekiq\n";
    assert_eq!(
        adapter(&[
            ("config/application.rb", &app),
            ("config/environments/production.rb", &prod),
            ("config/initializers/active_job.rb", init),
        ])
        .as_deref(),
        Some("sidekiq")
    );
}

#[test]
fn development_rb_is_not_read() {
    let dev = production("  config.active_job.queue_adapter = :sidekiq\n");
    assert_eq!(adapter(&[("config/environments/development.rb", &dev)]), None);
}

#[test]
fn a_non_literal_value_leaves_it_unknown() {
    let app = application("    config.active_job.queue_adapter = :sidekiq\n");
    let prod = production("  config.active_job.queue_adapter = ENV.fetch(\"QUEUE_ADAPTER\", \"sidekiq\").to_sym\n");
    assert_eq!(
        adapter(&[("config/application.rb", &app), ("config/environments/production.rb", &prod)]),
        None
    );
}

#[test]
fn solid_queue_is_stored_and_changes_nothing_else() {
    let prod = production("  config.active_job.queue_adapter = :solid_queue\n");
    let (app, _) = ingest(&[("config/environments/production.rb", &prod)]);
    assert_eq!(app.queue_adapter.as_deref(), Some("solid_queue"));
    assert!(app.job_plans.is_empty());
}

#[test]
fn the_ledger_reads_a_sidekiq_line_and_still_reports_other_adapters() {
    let ledger = |init: &str| -> Vec<String> {
        let (_, diags) = ingest(&[("config/initializers/active_job.rb", init)]);
        diags
            .into_iter()
            .filter_map(|d| match d.kind {
                DiagnosticKind::UnconsumedInitializer { statement, .. } => Some(statement.as_str().to_string()),
                _ => None,
            })
            .collect()
    };
    assert!(ledger("Rails.application.config.active_job.queue_adapter = :sidekiq\n").is_empty());
    assert_eq!(ledger("Rails.application.config.active_job.queue_adapter = :resque\n").len(), 1);
}
