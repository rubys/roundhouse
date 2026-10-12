//! Literal class-body `enum :name, …` (#30) as Rails documents it:
//! hash and keyword-hash mappings, `%i[]` / bracket array index,
//! `%w[].index_by` / `index_with` string identity, `prefix:` / `suffix:`
//! (`true` or a name), `default:` / `_default:`, `scopes: false`,
//! `instance_methods: false`, plus generated scopes, `not_` scopes,
//! predicates, bang writers, and the plural mapping. Overlays on
//! tiny-blog and real-blog — not a named-app special case.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::{MethodReceiver, ModelBodyItem};
use roundhouse::expr::Literal;
use roundhouse::ident::Symbol;
use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const TINY_SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "posts", force: :cascade do |t|
    t.string "title", null: false
    t.integer "state"
    t.integer "kind"
    t.string "theme"
    t.string "tone"
    t.integer "visibility"
    t.integer "role"
    t.integer "priority"
    t.integer "stage"
    t.integer "mood"
    t.integer "palette"
  end
  create_table "comments", force: :cascade do |t|
    t.text "body", null: false
    t.bigint "post_id", null: false
  end
end
"#;

fn post_app(model_body: &str) -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", TINY_SCHEMA),
        (
            "app/models/post.rb",
            &format!("class Post < ApplicationRecord\n  has_many :comments\n{model_body}\nend\n"),
        ),
        (
            "app/models/comment.rb",
            include_str!("../fixtures/tiny-blog/app/models/comment.rb"),
        ),
    ]))
    .expect("ingest")
}

fn post_model(app: &roundhouse::App) -> &roundhouse::dialect::Model {
    app.models
        .iter()
        .find(|m| m.name.0.as_str() == "Post")
        .expect("Post")
}

fn instance_names(app: &roundhouse::App) -> Vec<String> {
    post_model(app)
        .methods()
        .filter(|m| m.receiver == MethodReceiver::Instance)
        .map(|m| m.name.as_str().to_string())
        .collect()
}

fn class_names(app: &roundhouse::App) -> Vec<String> {
    post_model(app)
        .methods()
        .filter(|m| m.receiver == MethodReceiver::Class)
        .map(|m| m.name.as_str().to_string())
        .collect()
}

fn scope_names(app: &roundhouse::App) -> Vec<String> {
    post_model(app)
        .body
        .iter()
        .filter_map(|item| match item {
            ModelBodyItem::Scope { scope, .. } => Some(scope.name.as_str().to_string()),
            _ => None,
        })
        .collect()
}

fn has_name(names: &[String], want: &str) -> bool {
    names.iter().any(|n| n == want)
}

/// Hash mapping, `%i[]` / `[:a, :b]` index, `%w[].index_by` / `index_with`
/// identity, keyword-hash values, `prefix:` / `suffix:` true or a name,
/// `default:` / `_default:`, `scopes: false`, `instance_methods: false`,
/// plus the methods Rails generates from each.
#[test]
fn tiny_blog_rails_enum_syntaxes_ingest() {
    let app = post_app(
        r#"
  enum :state, { draft: 0, published: 1 }, default: :draft
  enum :kind, [:article, :note]
  enum :theme, %w[ black blue ].index_by(&:itself), suffix: true, default: :blue
  enum :tone, %w[ quiet loud ].index_with(&:itself), prefix: true
  enum :visibility, { open: 0, closed: 1 }, prefix: :vis
  enum status: { ready: "ready", blocked: "blocked" }, _suffix: :phase, _default: :ready
  enum :priority, low: 0, high: 1
  enum :stage, { queued: 0, live: 1 }, scopes: false
  enum :mood, %i[ calm tense ], instance_methods: false
  enum :palette, { red: 0, blue: 1 }, suffix: :ink
"#,
    );
    let post = post_model(&app);
    let inst = instance_names(&app);
    let scopes = scope_names(&app);
    let classes = class_names(&app);

    assert_eq!(
        post.enum_defaults.get(&Symbol::from("state")),
        Some(&Literal::Int { value: 0 })
    );
    assert_eq!(
        post.enum_defaults.get(&Symbol::from("theme")),
        Some(&Literal::Str {
            value: "blue".into()
        })
    );
    assert_eq!(
        post.enum_defaults.get(&Symbol::from("status")),
        Some(&Literal::Str {
            value: "ready".into()
        })
    );

    for name in [
        "draft?",
        "draft!",
        "published?",
        "article?",
        "note!",
        "blue_theme?",
        "black_theme!",
        "tone_quiet?",
        "tone_loud!",
        "vis_open?",
        "vis_closed!",
        "ready_phase?",
        "blocked_phase!",
        "low?",
        "high!",
        "queued?",
        "live!",
        "red_ink?",
        "blue_ink!",
    ] {
        assert!(has_name(&inst, name), "missing instance {name}: {inst:?}");
    }
    assert!(!has_name(&inst, "blue?"), "suffix: true must affix: {inst:?}");
    assert!(!has_name(&inst, "quiet?"), "prefix: true must affix: {inst:?}");
    assert!(!has_name(&inst, "open?"), "prefix: :vis must affix: {inst:?}");
    assert!(!has_name(&inst, "red?"), "suffix: :ink must affix: {inst:?}");
    assert!(
        !has_name(&inst, "calm?") && !has_name(&inst, "tense!"),
        "instance_methods: false must omit predicates/bang: {inst:?}"
    );

    for name in [
        "draft",
        "not_draft",
        "published",
        "not_published",
        "article",
        "not_note",
        "blue_theme",
        "not_black_theme",
        "tone_quiet",
        "not_tone_loud",
        "vis_open",
        "not_vis_closed",
        "ready_phase",
        "not_blocked_phase",
        "low",
        "not_high",
        "calm",
        "not_tense",
        "red_ink",
        "not_blue_ink",
    ] {
        assert!(has_name(&scopes, name), "missing scope {name}: {scopes:?}");
    }
    assert!(
        !has_name(&scopes, "queued") && !has_name(&scopes, "not_live"),
        "scopes: false must omit scopes: {scopes:?}"
    );
    for name in [
        "states",
        "kinds",
        "themes",
        "tones",
        "visibilities",
        "statuses",
        "priorities",
        "stages",
        "moods",
        "palettes",
    ] {
        assert!(has_name(&classes, name), "missing mapping {name}: {classes:?}");
    }
}

/// `%i[…], default:` in a concern `included do` folds mapping and
/// `enum_defaults` onto the includer — ingest pin, not only emit_and_run.
#[test]
fn tiny_blog_included_enum_default_ingests() {
    let app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", TINY_SCHEMA),
        (
            "app/models/concerns/assignable.rb",
            r#"module Assignable
  extend ActiveSupport::Concern
  included do
    enum :role, %i[ member administrator ], default: :member
  end
end
"#,
        ),
        (
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  include Assignable\nend\n",
        ),
        (
            "app/models/comment.rb",
            include_str!("../fixtures/tiny-blog/app/models/comment.rb"),
        ),
    ]))
    .expect("ingest");
    let post = post_model(&app);
    assert_eq!(
        post.enum_defaults.get(&Symbol::from("role")),
        Some(&Literal::Int { value: 0 })
    );
    let inst = instance_names(&app);
    assert!(has_name(&inst, "member?"), "{inst:?}");
    assert!(has_name(&inst, "administrator!"), "{inst:?}");
    let scopes = scope_names(&app);
    assert!(has_name(&scopes, "member"), "{scopes:?}");
    assert!(has_name(&scopes, "not_administrator"), "{scopes:?}");
}

#[test]
fn emitted_rails_enum_syntaxes_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            r#"class Article < ApplicationRecord
  enum :state, { draft: 0, published: 1 }, default: :draft
  enum :kind, [:article, :note]
  enum :theme, %w[ black blue green ].index_by(&:itself), suffix: true, default: :blue
  enum :tone, { quiet: "q", loud: "l" }, prefix: true
  enum :visibility, { open: 0, closed: 1 }, prefix: :vis
  enum :priority, low: 0, high: 1
  enum :stage, { queued: 0, live: 1 }, scopes: false
  enum :mood, %i[ calm tense ], instance_methods: false
  enum :palette, { red: 0, blue: 1 }, suffix: :ink
"#,
        )
        .edit(
            "db/schema.rb",
            "    t.string \"title\"",
            r#"    t.string "title"
    t.integer "state"
    t.integer "kind"
    t.string "theme"
    t.string "tone"
    t.integer "visibility"
    t.integer "priority"
    t.integer "stage"
    t.integer "mood"
    t.integer "palette""#,
        )
        .run_ruby(
            r#"
a = Article.new(title: "Hello world", body: "abcdefghij")
raise "hash default" unless a.draft?
raise "hash not published" if a.published?
raise "index_by suffix default" unless a.blue_theme?
raise "unsuffixed" if a.respond_to?(:blue?)
raise "prefix unset" if a.tone_quiet? || a.tone_loud?
raise "custom prefix unset" if a.vis_open? || a.vis_closed?
raise "kwargs unset" if a.low? || a.high?
raise "scopes:false missing predicate" unless a.respond_to?(:queued?)
raise "scopes:false leaked" if Article.respond_to?(:queued)
raise "instance_methods:false leaked" if a.respond_to?(:calm?) || a.respond_to?(:tense!)
raise "named suffix unset" if a.respond_to?(:red?) || a.red_ink?

a.published!
raise "hash bang" unless a.published?
raise "hash stored label" unless a.state == "published"
a.save!
raise "hash scope after save" unless Article.published.where(id: a.id).exists?
raise "not_ scope" unless Article.not_draft.where(id: a.id).exists?

a.note!
raise "bracket-array bang" unless a.note?
raise "bracket-array stored" unless a.kind == "note"

a.green_theme!
raise "suffix bang" unless a.green_theme?
raise "string stored" unless a.theme == "green"
raise "suffix scope" unless Article.green_theme.where(id: a.id).exists?

a.tone_loud!
raise "prefix bang" unless a.tone_loud?
raise "unprefixed" if a.respond_to?(:loud?)
raise "prefix stored" unless a.tone == "loud"
raise "prefix scope" unless Article.tone_loud.where(id: a.id).exists?
raise "prefix not_ scope" unless Article.not_tone_quiet.where(id: a.id).exists?

a.vis_closed!
raise "named prefix bang" unless a.vis_closed?
raise "named prefix stored" unless a.visibility == "closed"
raise "named prefix scope" unless Article.vis_closed.where(id: a.id).exists?

a.high!
raise "kwargs bang" unless a.high?
raise "kwargs stored" unless a.priority == "high"
raise "kwargs scope" unless Article.high.where(id: a.id).exists?

a.live!
raise "scopes:false bang" unless a.live?
raise "scopes:false still leaked" if Article.respond_to?(:live) || Article.respond_to?(:not_queued)

a.mood = "tense"
a.save!
raise "instance_methods:false assignment" unless a.mood == "tense"
raise "instance_methods:false scope" unless Article.tense.where(id: a.id).exists?

a.blue_ink!
raise "named suffix bang" unless a.blue_ink?
raise "named suffix stored" unless a.palette == "blue"
raise "named suffix scope" unless Article.blue_ink.where(id: a.id).exists?

puts "rails enum syntaxes passed"
"#,
        )
        .assert_passes();
}

/// `%i[…], default:` declared in a concern `included do` reaches the
/// includer as ordinary predicates, bang writers, and the default.
#[test]
fn emitted_included_integer_enum_runs() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/assignable.rb",
            r#"module Assignable
  extend ActiveSupport::Concern

  included do
    enum :role, %i[ member administrator ], default: :member
  end

  def can_administer?
    administrator?
  end
end
"#,
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Assignable\n",
        )
        .edit(
            "db/schema.rb",
            "    t.string \"title\"",
            "    t.string \"title\"\n    t.integer \"role\"",
        )
        .run_ruby(
            r#"
a = Article.new(title: "Hello world", body: "abcdefghij")
raise "included default" unless a.member?
raise "helper" if a.can_administer?
a.administrator!
raise "bang" unless a.can_administer?
raise "stored label" unless a.role == "administrator"
a.save!
raise "included scope" unless Article.administrator.where(id: a.id).exists?
raise "included not_ scope" unless Article.not_member.where(id: a.id).exists?
puts "included integer enum passed"
"#,
        )
        .assert_passes();
}

/// A role enum's spelling: `ROLE_B = ROLE_A`, so the mapping's
/// first two keys fold to the same string. Ruby builds one entry, in the
/// first key's position with the last value; the generated `kinds`
/// literal must too, or Ruby warns "key duplicated" when the model file
/// is parsed (a warnings-as-errors boot raises it at load).
fn folded_label_enum_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            r#"class Article < ApplicationRecord
  ROLE_A = "role_a"
  ROLE_B = ROLE_A
  ROLE_C = "role_c"
  enum :kind, { ROLE_B => "role_b", ROLE_C => ROLE_C, ROLE_A => ROLE_A }
"#,
        )
        .edit("db/schema.rb", "    t.string \"title\"", "    t.string \"title\"\n    t.string \"kind\"")
}

#[test]
fn constant_labels_folding_to_one_key_build_one_entry() {
    let (tree, _errors) = folded_label_enum_app().emit(roundhouse::project::BuildTarget::Ruby);
    let check = emit_and_run::ruby().arg("-wc").arg(tree.join("app/models/article.rb")).output().expect("ruby -wc");
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(check.status.success() && !stderr.contains("duplicated"), "{stderr}");

    folded_label_enum_app()
        .run_ruby(
            r#"
expected = { "role_a" => "role_a", "role_c" => "role_c" }
raise "kinds: #{Article.kinds.inspect}" unless Article.kinds == expected && Article.kinds.keys == expected.keys
a = Article.new(title: "Hello world", body: "abcdefghij", kind: "role_a")
raise "predicate" unless a.role_a?
"#,
        )
        .assert_passes();
}
