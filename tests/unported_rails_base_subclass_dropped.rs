//! A class extending a Rails base the runtime does not port is dropped
//! after analysis, out loud, with its subclasses — and kept for `check`.
//!
//! Rails autoloads every `app/*` subdirectory and so does ingest, which
//! is how lobsters' `app/mailboxes/` arrived: `ApplicationMailbox <
//! ActionMailbox::Base` was carried as a library class and replayed
//! verbatim (the rule for a gem's DSL base), and the emitted tree then
//! raised `NameError: uninitialized constant ActionMailbox` from
//! `app/models.rb` — no spec ran. There is no `ActionMailbox` in
//! `runtime/ruby/`, and a stub base would pretend an inbound-email
//! pipeline exists. The emit-bound drivers drop the class instead,
//! transitively, and a `lower_residue` line names the base; the doors
//! that run the analyzer without the lowerings (`check`, LSP, MCP)
//! still type the mailbox's `process` like any other method.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "comments", force: :cascade do |t|
    t.string "body", null: false
  end
end
"#;

const APPLICATION_MAILBOX: &str = r#"class ApplicationMailbox < ActionMailbox::Base
  routing all: :inbox
end
"#;

const INBOX_MAILBOX: &str = r#"class InboxMailbox < ApplicationMailbox
  def process
    Comment.create!(body: "hi")
  end
end
"#;

const CURRENT: &str = r#"class Current < ActiveSupport::CurrentAttributes
  attribute :user
end
"#;

const SERVICE: &str = r#"class Greeter
  def self.call
    "hello"
  end
end
"#;

fn ingest(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [("db/schema.rb", SCHEMA)]
        .iter()
        .chain(files.iter())
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

fn library_class_names(app: &roundhouse::App) -> Vec<&str> {
    let mut names: Vec<&str> =
        app.library_classes.iter().map(|lc| lc.name.0.as_str()).collect();
    names.sort();
    names
}

fn dropped(app: &roundhouse::App, diags: &[roundhouse::diagnostic::Diagnostic]) -> Vec<String> {
    diags
        .iter()
        .map(|d| d.render(&app.sources))
        .filter(|d| d.contains("class dropped"))
        .collect()
}

#[test]
fn a_mailbox_and_its_subclass_are_dropped_at_lowering_and_ledgered() {
    let mut app = ingest(&[
        // The subclass sorts first, so ingest reads it before its
        // parent: the drop has to resolve the chain over the whole set.
        ("app/mailboxes/a_inbox_mailbox.rb", INBOX_MAILBOX),
        ("app/mailboxes/application_mailbox.rb", APPLICATION_MAILBOX),
        ("app/services/greeter.rb", SERVICE),
    ]);
    // Ingest keeps them: the analyzer-only doors see the mailbox.
    assert_eq!(
        library_class_names(&app),
        vec!["ApplicationMailbox", "Greeter", "InboxMailbox"]
    );

    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert_eq!(library_class_names(&app), vec!["Greeter"]);

    let gaps = dropped(&app, &diags);
    assert_eq!(gaps.len(), 2, "{gaps:?}");
    assert!(
        gaps.iter().any(|g| g.contains("`ApplicationMailbox` extends `ActionMailbox::Base`")),
        "{gaps:?}"
    );
    assert!(
        gaps.iter().any(|g| g
            .contains("`InboxMailbox` extends `ActionMailbox::Base` through `ApplicationMailbox`")
            && g.contains("a_inbox_mailbox.rb")),
        "{gaps:?}"
    );
    assert!(gaps.iter().all(|g| g.contains("warning[lower_residue]")), "{gaps:?}");

    // And nothing in the emitted library names the framework: the
    // tree loads.
    let out = emit_library(&app)
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!out.contains("Mailbox"), "{out}");
}

#[test]
fn a_ported_rails_base_is_kept() {
    let mut app = ingest(&[("app/models/current.rb", CURRENT)]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(dropped(&app, &diags).is_empty());
    assert_eq!(library_class_names(&app), vec!["Current"]);
}

/// A gem's base parked under a Rails namespace is not a Rails base:
/// the class stays on the gem-DSL path (kept, body replayed), as it
/// was before.
#[test]
fn a_gem_base_under_a_rails_namespace_is_kept() {
    let mut app = ingest(&[(
        "app/serializers/comment_serializer.rb",
        "class CommentSerializer < ActiveModel::Serializer\n  attributes :body\nend\n",
    )]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(dropped(&app, &diags).is_empty());
    assert_eq!(library_class_names(&app), vec!["CommentSerializer"]);
}
