//! `extend self` — the other spelling of `module_function`.
//!
//! Ruby makes every instance method of the module a singleton method
//! too, so `PrivateNetworkGuard.resolve(host)` reaches the `def resolve`
//! written without a receiver. Dropped at ingest, the module emitted
//! instance-only methods and every dotted call was a NoMethodError.
//!
//! campfire paid for this a long way from the declaration:
//! `Opengraph::Metadata.from_url` fetches through
//! `RestrictedHTTP::PrivateNetworkGuard.resolve`, so with the module
//! silent the whole fetch chain returned an EMPTY document — and the
//! tests that asserted on the result passed anyway, because nothing
//! else checked that the object had been filled in.

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

fn emitted(lib_src: &str) -> String {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n",
        ),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("app/models/guard.rb", lib_src),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.ends_with("guard.rb"))
        .expect("no guard.rb emitted")
        .content
        .clone()
}

#[test]
fn extend_self_makes_every_def_callable_on_the_module() {
    let src = emitted(
        r#"module Guard
  extend self

  def resolve(host)
    host
  end

  def private?(ip)
    false
  end
end
"#,
    );
    assert!(src.contains("def self.resolve(host)"), "{src}");
    assert!(src.contains("def self.private?(ip)"), "{src}");
}

/// Without it the methods stay instance-only — the behaviour every
/// other module keeps.
#[test]
fn a_module_without_extend_self_keeps_instance_methods() {
    let src = emitted(
        r#"module Guard
  def resolve(host)
    host
  end
end
"#,
    );
    assert!(src.contains("def resolve(host)"), "{src}");
    assert!(!src.contains("def self.resolve"), "{src}");
}

/// `extend SomethingElse` is a plain mixin and must not flip anything.
#[test]
fn extend_of_another_module_is_not_extend_self() {
    let src = emitted(
        r#"module Guard
  extend Comparable

  def resolve(host)
    host
  end
end
"#,
    );
    assert!(!src.contains("def self.resolve"), "{src}");
}
