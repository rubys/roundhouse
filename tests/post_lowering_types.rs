//! Precision the post-lowering diagnose depends on.
//!
//! Each case was an error diagnostic on lobsters whose emitted Ruby ran
//! correctly (the `ruby` lane renders those pages to parity with Rails),
//! so the error was the analysis's, not the app's. Under `--target
//! futamura`'s whole-route replacement an error keeps every route that
//! reaches it on Rails, which made these three worth a regression test
//! each: two of them sat on code every lobsters request runs.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "keystores", force: :cascade do |t|
    t.string "key", null: false
    t.bigint "value"
  end
end
"#;

fn errors(extra: &[(&str, &str)]) -> Vec<String> {
    let mut all: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        (
            "app/models/keystore.rb",
            "class Keystore < ApplicationRecord\n  def self.value_for(key)\n    self.where(key: key).limit(1).pluck(:value).first\n  end\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/\" => \"home#index\"\nend\n"),
        ("app/views/home/index.html.erb", "<p><%= @style %></p>\n"),
    ];
    all.extend_from_slice(extra);
    let tree: HashMap<PathBuf, Vec<u8>> =
        all.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == roundhouse::analyze::Severity::Error)
        .map(|d| d.message)
        .collect()
}

/// `pluck(:nullable).first` is `(Integer | nil) | nil`; `|| 0.5` must
/// peel both nils, so the arithmetic after it is on a number. lobsters'
/// `TrafficHelper.cached_current_intensity`, read by the
/// `set_traffic_style` filter on every request.
#[test]
fn or_default_peels_a_nested_nullable() {
    let errs = errors(&[(
        "app/controllers/home_controller.rb",
        "class HomeController < ApplicationController\n  def index\n    intensity = Keystore.value_for(\"traffic\") || 0.5\n    @style = sprintf(\"%02x\", (intensity * 1.75 + 80).round)\n  end\nend\n",
    )]);
    assert!(errs.is_empty(), "{errs:#?}");
}

/// `Rails.cache.fetch(key, expires_in: 60 * 2) { … }` lowers to
/// `fetch_str(key, (60 * 2).to_i)`; the `to_i` the pass builds is typed,
/// not read as a failed dispatch on Integer.
#[test]
fn the_cache_lowering_types_the_ttl_it_builds() {
    let errs = errors(&[(
        "app/controllers/home_controller.rb",
        "class HomeController < ApplicationController\n  def index\n    @style = Rails.cache.fetch(\"style\", :expires_in => (60 * 2)) { \"plain\" }\n  end\nend\n",
    )]);
    assert!(errs.is_empty(), "{errs:#?}");
}
