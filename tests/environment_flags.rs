//! `config.assume_ssl = true` in an environment file reaches the emitted
//! app as `AppEnvironment.assume_ssl?(env)` in boot.rb.
//!
//! hindmost sets it in production: TLS ends at Tailscale, which forwards
//! plain http, and Rails' AssumeSSL treats every request as https.

use std::collections::HashMap;
use std::path::PathBuf;

fn boot_rb(production_rb: &str) -> String {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("config/environments/production.rb", production_rb),
        ("config/environments/development.rb", "Rails.application.configure do\n  config.assume_ssl = false\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, roundhouse::fixtures::real_blog())
        .expect("spinel tree");
    files.into_iter().find(|(p, _)| p == "boot.rb").map(|(_, c)| c).expect("boot.rb")
}

#[test]
fn assume_ssl_is_carried_for_the_environment_that_sets_it() {
    let boot = boot_rb("Rails.application.configure do\n  config.assume_ssl = true\nend\n");
    assert!(boot.contains("module AppEnvironment\n  def self.assume_ssl?(env)\n    env == \"production\"\n  end\nend"), "{boot}");
}

#[test]
fn a_commented_assume_ssl_is_not() {
    let boot = boot_rb("Rails.application.configure do\n  # config.assume_ssl = true\nend\n");
    assert!(boot.contains("def self.assume_ssl?(env)\n    false\n"), "{boot}");
}

#[test]
fn an_assume_ssl_with_a_trailing_comment_is_carried() {
    let boot = boot_rb("Rails.application.configure do\n  config.assume_ssl = true # TLS ends at the proxy\nend\n");
    assert!(boot.contains("def self.assume_ssl?(env)\n    env == \"production\"\n"), "{boot}");
}
