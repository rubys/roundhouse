//! lobsters' Token concern, end to end through analyze + lower + the
//! spinel emit:
//!
//!     after_initialize do
//!       self.token ||= TypeID.new(self.class.to_s.parameterize) if new_record? || has_attribute?(:token)
//!     end
//!
//! Three things had to hold for a LOADED record, and none did:
//!
//! * the hook runs once, after the columns are set — the hydration
//!   factories used to construct through `new`, whose tail fired the
//!   hook on the empty shell (token blank, so it minted one) before the
//!   factory set the columns and fired it again;
//! * inside the hook the record is already persisted (`new_record?`
//!   false), as Rails has it on a find, and a column the query did not
//!   select answers `has_attribute?` false (`instantiate` notes them
//!   before it fires the hook);
//! * `parameterize` inside the block-form hook is grounded to
//!   `Inflector.parameterize` — the grounding walked `def` methods only,
//!   and spinel has no String#parameterize to dispatch.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted_model() -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"hats\", force: :cascade do |t|\n    t.string \"token\", null: false\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "app/models/concerns/token.rb",
            "module Token\n  extend ActiveSupport::Concern\n\n  included do\n    after_initialize do\n      self.token ||= TypeID.new(self.class.to_s.parameterize) if new_record? || has_attribute?(:token)\n    end\n  end\nend\n",
        ),
        ("app/models/hat.rb", "class Hat < ApplicationRecord\n  include Token\nend\n"),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with("app/models/hat.rb"))
        .expect("hat.rb emitted")
        .content
        .clone()
}

/// The body of `def <name>` up to its closing `end` at the same indent.
fn method_body<'a>(src: &'a str, header: &str) -> &'a str {
    let start = src.find(header).unwrap_or_else(|| panic!("no `{header}` in:\n{src}"));
    let rest = &src[start..];
    let end = rest.find("\n  end\n").unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn hydration_factories_fire_the_hook_once_on_a_persisted_record() {
    let src = emitted_model();
    for factory in ["def self.from_row(", "def self.from_stmt("] {
        let body = method_body(&src, factory);
        assert!(
            body.contains("Hat.new(ActiveRecord::Base::HYDRATE_ATTRS)"),
            "{factory} constructs with the sentinel:\n{body}"
        );
    }
    assert!(
        !method_body(&src, "def self.from_row(").contains("after_initialize"),
        "from_row leaves the hook to instantiate"
    );
    for (factory, before) in [
        ("def self.from_stmt(", vec!["mark_persisted!"]),
        ("def self.instantiate(", vec!["mark_persisted!", "_note_unloaded(row)"]),
    ] {
        let body = method_body(&src, factory);
        assert_eq!(body.matches("after_initialize").count(), 1, "{factory} fires once:\n{body}");
        let hook = body.find("after_initialize").unwrap();
        for step in before {
            let at = body.find(step).unwrap_or_else(|| panic!("{factory} does {step}:\n{body}"));
            assert!(at < hook, "{factory}: {step} before the hook:\n{body}");
        }
    }
    let init = method_body(&src, "def initialize(");
    assert!(
        init.contains("after_initialize if !(attrs.equal? ActiveRecord::Base::HYDRATE_ATTRS)"),
        "initialize skips the hook for the hydration sentinel:\n{init}"
    );
}

#[test]
fn parameterize_in_a_block_form_hook_is_grounded() {
    let src = emitted_model();
    let hook = method_body(&src, "def after_initialize");
    assert!(
        hook.contains("Inflector.parameterize(self.class.to_s)"),
        "grounded to the runtime Inflector:\n{hook}"
    );
    assert!(
        !hook.replace("Inflector.parameterize(", "").contains(".parameterize"),
        "no String#parameterize dispatch left:\n{hook}"
    );
}
