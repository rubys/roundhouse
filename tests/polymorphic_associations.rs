//! Polymorphic associations (`belongs_to :x, polymorphic: true` +
//! `has_many/has_one …, as: :x`).
//!
//! The belongs_to target set is resolved at ingest assembly
//! (`resolve_polymorphic_targets`) from two sources: the inverse `as:`
//! declarations (Rails-canonical), and — when no model declares one —
//! literal `<x>_type` mentions in the owner's own body
//! (`where(item_type: "Moderation")` hashes, raw-SQL join fragments).
//! The lowering then synthesizes a type-switched reader, a writer that
//! stores the (type, id) pair, and inverse readers scoped by the type
//! column. Both shapes are lobsters HEAD idioms (Notification's
//! notifiable, ModActivity's item).

use roundhouse::dialect::Association;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_models_with_registry_and_params;

fn app_from(files: Vec<(&str, &str)>) -> roundhouse::App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :notifications do |t|\n    t.integer :user_id\n    t.string :notifiable_type\n    t.integer :notifiable_id\n  end\n  create_table :comments do |t|\n    t.text :body\n  end\n  create_table :messages do |t|\n    t.text :body\n  end\nend\n";

fn notification_app() -> roundhouse::App {
    app_from(vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/notification.rb",
            "class Notification < ApplicationRecord\n  belongs_to :notifiable, polymorphic: true\nend\n",
        ),
        (
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n  has_many :notifications, as: :notifiable\nend\n",
        ),
        (
            "app/models/message.rb",
            "class Message < ApplicationRecord\n  has_one :notification, as: :notifiable\nend\n",
        ),
    ])
}

#[test]
fn polymorphic_targets_resolve_from_inverse_as_decls() {
    let app = notification_app();
    let notification = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Notification")
        .expect("Notification model");
    let assoc = notification.associations().next().expect("belongs_to");
    let Association::BelongsTo { polymorphic, polymorphic_targets, foreign_key, .. } = assoc
    else {
        panic!("expected BelongsTo, got {assoc:?}");
    };
    assert!(polymorphic, "polymorphic: true must be ingested");
    assert_eq!(foreign_key.as_str(), "notifiable_id");
    let names: Vec<&str> = polymorphic_targets.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(names, vec!["Comment", "Message"], "targets from inverse as: decls");
}

#[test]
fn as_interface_defaults_foreign_key_to_interface_id() {
    let app = notification_app();
    let comment = app.models.iter().find(|m| m.name.0.as_str() == "Comment").unwrap();
    let Association::HasMany { foreign_key, as_interface, .. } =
        comment.associations().next().expect("has_many")
    else {
        panic!("expected HasMany");
    };
    assert_eq!(foreign_key.as_str(), "notifiable_id", "fk from as:, not owner name");
    assert_eq!(as_interface.as_ref().map(|s| s.as_str()), Some("notifiable"));
}

#[test]
fn polymorphic_reader_switches_on_type_and_writer_stores_pair() {
    let app = notification_app();
    let (lcs, _registry) = lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        vec![],
        &Default::default(),
    );
    let notification =
        lcs.iter().find(|lc| lc.name.0.as_str() == "Notification").expect("lowered");

    let body_of = |name: &str| {
        let m = notification
            .methods
            .iter()
            .find(|m| m.name.as_str() == name)
            .unwrap_or_else(|| panic!("`{name}` not synthesized"));
        format!("{:?}", m.body)
    };

    // Reader: case @notifiable_type when "Comment"/"Message" → a
    // per-target lookup by @notifiable_id, else nil. (The Level-3
    // adapter pass rewrites the synthesized find_by into direct SQL,
    // so assert the case skeleton + per-arm target tables.)
    let reader = body_of("notifiable");
    for needle in [
        "Case",
        "notifiable_type",
        "\"Comment\"",
        "\"Message\"",
        "notifiable_id",
        "FROM comments",
        "FROM messages",
    ] {
        assert!(reader.contains(needle), "reader must contain {needle}: {reader}");
    }

    // Writer: stores both halves of the pair.
    let writer = body_of("notifiable=");
    for needle in ["notifiable_id", "notifiable_type", "\"Comment\"", "\"Message\""] {
        assert!(writer.contains(needle), "writer must contain {needle}: {writer}");
    }
}

#[test]
fn as_scoped_inverse_readers_carry_the_type_condition() {
    let app = notification_app();
    let (lcs, _registry) = lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        vec![],
        &Default::default(),
    );
    let body_of = |class: &str, name: &str| {
        let lc = lcs.iter().find(|lc| lc.name.0.as_str() == class).expect("lowered");
        let m = lc
            .methods
            .iter()
            .find(|m| m.name.as_str() == name)
            .unwrap_or_else(|| panic!("`{class}#{name}` not synthesized"));
        format!("{:?}", m.body)
    };

    // has_many: … WHERE notifiable_id = @id AND notifiable_type =
    // 'Comment' (adapter pass renders the where-hash as SQL; the type
    // literal arrives single-quoted).
    let many = body_of("Comment", "notifications");
    for needle in ["notifiable_id", "notifiable_type", "'Comment'"] {
        assert!(many.contains(needle), "as:-scoped has_many must contain {needle}: {many}");
    }

    // has_one: same pair, owner Message.
    let one = body_of("Message", "notification");
    for needle in ["notifiable_id", "notifiable_type", "'Message'"] {
        assert!(one.contains(needle), "as:-scoped has_one must contain {needle}: {one}");
    }
}

#[test]
fn polymorphic_targets_resolve_from_owner_body_literals() {
    // No model declares `as: :item` — the set comes from the owner's
    // own literals: a raw-SQL join fragment and a where-hash condition
    // (ModActivity's exact idiom).
    let app = app_from(vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :mod_activities do |t|\n    t.string :item_type\n    t.integer :item_id\n  end\n  create_table :moderations do |t|\n    t.string :action\n  end\n  create_table :mod_notes do |t|\n    t.text :note\n  end\nend\n",
        ),
        (
            "app/models/mod_activity.rb",
            r#"class ModActivity < ApplicationRecord
  belongs_to :item, polymorphic: true

  scope :with_item, -> {
    joins("left outer join moderations on (item_type = 'Moderation' and item_id = moderations.id)")
      .where(item_type: "ModNote")
  }
end
"#,
        ),
        (
            "app/models/moderation.rb",
            "class Moderation < ApplicationRecord\nend\n",
        ),
        (
            "app/models/mod_note.rb",
            "class ModNote < ApplicationRecord\nend\n",
        ),
    ]);
    let mod_activity =
        app.models.iter().find(|m| m.name.0.as_str() == "ModActivity").unwrap();
    let Association::BelongsTo { polymorphic_targets, .. } =
        mod_activity.associations().next().expect("belongs_to")
    else {
        panic!("expected BelongsTo");
    };
    let names: Vec<&str> = polymorphic_targets.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(
        names,
        vec!["Moderation", "ModNote"],
        "targets from SQL fragment + where-hash literals"
    );
}
