//! `class Current < ActiveSupport::CurrentAttributes` becomes a plain,
//! statically-resolvable class (`ingest::current_attributes`).
//!
//! Rails stacks three pieces of metaprogramming here — `attribute`
//! defining accessors into a generated module, class-level `Current.user`
//! arriving via `method_missing`, and an app's own writer reaching the
//! generated one through `super`. An emitted tree has none of it, and a
//! statically-resolved target cannot follow `method_missing` at all.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn current() -> String {
    let app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\" do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
        ("app/models/account.rb", "class Account < ApplicationRecord\nend\n"),
        (
            "app/models/current.rb",
            r#"class Current < ActiveSupport::CurrentAttributes
  attribute :session, :user, :request

  delegate :host, :protocol, to: :request, prefix: true, allow_nil: true

  def session=(value)
    super(value)

    if value.present?
      self.user = session.user
    end
  end

  def account
    Account.first
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("current.rb"))
        .map(|f| f.content.clone())
        .expect("current.rb")
}

/// The base is gone — nothing in an emitted tree provides it.
#[test]
fn the_current_attributes_base_is_dropped() {
    let c = current();
    assert!(!c.contains("ActiveSupport::CurrentAttributes"), "{c}");
    assert!(c.contains("class Current\n"), "{c}");
}

/// `attribute :session, :user, :request` becomes real ivar accessors.
#[test]
fn declared_attributes_become_accessors() {
    let c = current();
    for m in ["def user\n    @user", "def user=(value)\n    @user = value", "def request\n    @request"] {
        assert!(c.contains(m), "missing {m:?}:\n{c}");
    }
}

/// The app writes its OWN `session=`, so only the reader is synthesized —
/// and its `super(value)` becomes the storage write it means, rather than
/// needing a module for `super` to find.
#[test]
fn an_app_written_writer_keeps_its_body_and_loses_super() {
    let c = current();
    assert!(!c.contains("super("), "no super survives:\n{c}");
    assert!(
        c.contains("def session=(value)\n    @session = value"),
        "super became the storage write:\n{c}"
    );
    assert_eq!(c.matches("def session=").count(), 1, "not double-defined:\n{c}");
}

/// `delegate … prefix: true, allow_nil: true` — the nil guard is the
/// whole point of `allow_nil`.
#[test]
fn delegate_expands_with_its_nil_guard() {
    let c = current();
    assert!(c.contains("def request_host"), "prefixed name:\n{c}");
    assert!(c.contains("@request.nil?"), "allow_nil guard:\n{c}");
}

/// The surface app code actually calls. Rails supplies these through
/// `method_missing`; a statically-resolved target needs them real —
/// including for a method the app defined itself (`account`).
#[test]
fn class_level_forwarders_exist_for_every_instance_method() {
    let c = current();
    for m in [
        "def self.user\n    Current.instance.user",
        "def self.user=(value)\n    Current.instance.user = value",
        "def self.account\n    Current.instance.account",
        "def self.request_host",
        "def self.instance",
        "def self.reset",
    ] {
        assert!(c.contains(m), "missing {m:?}:\n{c}");
    }
}
