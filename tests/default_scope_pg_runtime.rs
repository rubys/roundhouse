//! PostgreSQL: default scope, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE shelves (
    id bigserial NOT NULL,
    CONSTRAINT shelves_pkey PRIMARY KEY (id)
);

CREATE TABLE docs (
    id bigserial NOT NULL,
    shelf_id bigint,
    name character varying,
    is_not_deleted boolean,
    CONSTRAINT docs_pkey PRIMARY KEY (id)
);

CREATE TABLE tags (
    id bigserial NOT NULL,
    title character varying,
    status character varying,
    CONSTRAINT tags_pkey PRIMARY KEY (id)
);

CREATE TABLE badges (
    id bigserial NOT NULL,
    title character varying,
    status character varying,
    archived boolean,
    CONSTRAINT badges_pkey PRIMARY KEY (id)
);

CREATE TABLE rules (
    id bigserial NOT NULL,
    rule_type integer,
    title character varying,
    CONSTRAINT rules_pkey PRIMARY KEY (id)
);

CREATE TABLE notes (
    id bigserial NOT NULL,
    type character varying,
    title character varying,
    status character varying,
    CONSTRAINT notes_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_default_scope
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHELF: &str = r#"class Shelf < ApplicationRecord
  has_many :docs

  def self.doc_counts
    Shelf.includes(:docs).order(:id).map { |s| s.docs.size }.join(",")
  end
end
"#;

const DOC: &str = r#"class Doc < ApplicationRecord
  default_scope { where(is_not_deleted: true) }
  belongs_to :shelf, optional: true

  def self.listed
    Doc.order(:name).pluck(:name).join(",")
  end
end
"#;

const TAG_FINDERS: &str = r#"module TagFinders
  def live
    where.not(status: "removed")
  end
end
"#;

const TAG: &str = r#"class Tag < ApplicationRecord
  default_scope { order(:title) }
  default_scope { extending(TagFinders) }
end
"#;

const NOTE: &str = r#"class Note < ApplicationRecord
  default_scope { where(status: "live") }
end
"#;

/// An STI subclass's own default_scope, after its parent's.
const PINNED: &str = r#"class Pinned < Note
  default_scope { order(:title) }
end
"#;

/// A default scope of named scopes, one declared after it (Rails runs
/// the block lazily).
const BADGE: &str = r#"class Badge < ApplicationRecord
  scope :kept, -> { where(archived: false) }
  default_scope { kept.shown }
  scope :shown, -> { where.not(status: "hidden") }
end
"#;

const RULE: &str = r#"class Rule < ApplicationRecord
  MARKDOWN = 2
end
"#;

/// A model's subclass on a table with no `type` column: no STI, but its
/// own default scope (on the parent's constant) still applies.
const MARKDOWN_RULE: &str = r#"class MarkdownRule < Rule
  default_scope { where(rule_type: MARKDOWN) }
end
"#;

const PROBE: &str = r##"class DefaultScopeProbe
  def self.names(docs)
    docs.map(&:name).sort.join(",")
  end

  def self.lines
    out = []
    shelf = Shelf.create!
    Doc.create!(name: "a", shelf: shelf)
    Doc.create!(name: "b", shelf: shelf)
    gone = Doc.unscoped.create!(name: "gone", shelf: shelf, is_not_deleted: false)
    fresh = Doc.new(name: "n")
    out << "new: #{fresh.is_not_deleted.inspect} #{Doc.unscoped.new.is_not_deleted.inspect}"
    out << "all: #{Doc.count} #{names(Doc.all)} #{Doc.where(name: "gone").count} #{Doc.exists?(gone.id)}"
    found = begin
      Doc.find(gone.id).name
    rescue ActiveRecord::RecordNotFound => e
      e.class.name
    end
    out << "find: #{found} #{Doc.find_by(name: "gone").inspect} #{Doc.find_by(name: "a").name}"
    out << "unscoped: #{Doc.unscoped.count} #{Doc.unscoped.find(gone.id).name} #{Doc.unscoped { Doc.count }} #{Doc.count}"
    out << "association: #{shelf.docs.count} #{names(shelf.docs)} #{shelf.docs.where(name: "gone").exists?}"
    out << "arel: #{Doc.listed}"
    out << "belongs_to: #{gone.shelf == shelf}"
    out << "preload: #{names(Shelf.preload(:docs).find(shelf.id).docs)} #{Shelf.includes(:docs).where(id: shelf.id).to_a.first.docs.size} #{Shelf.doc_counts}"
    out << "joins: #{Shelf.joins(:docs).count}"
    partial = Doc.select(:id, :name).find_by(name: "a")
    missing = begin
      partial.is_not_deleted.inspect
    rescue NoMethodError => e
      e.class.name
    end
    out << "select: #{partial.name} #{partial.has_attribute?(:is_not_deleted)} #{missing}"
    gone.update!(is_not_deleted: true)
    out << "restored: #{Doc.count} #{names(Doc.where(shelf_id: shelf.id))}"
    Tag.create!(title: "c", status: "live")
    Tag.create!(title: "a", status: "removed")
    Tag.create!(title: "b", status: "live")
    out << "order: #{Tag.all.map(&:title).join(",")} #{Tag.first.title} #{Tag.last.title} #{Tag.reorder(:id).first.title}"
    out << "extending: #{Tag.all.live.map(&:title).join(",")} #{Tag.where(status: "live").live.count}"
    Note.create!(title: "z")
    Pinned.create!(title: "y")
    Pinned.create!(title: "x")
    Pinned.unscoped.create!(title: "w", status: "draft")
    out << "sti: #{Pinned.new.status} #{Pinned.all.map(&:title).join(",")} #{Pinned.count} #{Note.count} #{Pinned.unscoped.count} #{Note.unscoped.count} #{Pinned.first.title}"
    q = Doc.where(name: "q").new
    made = shelf.docs.where(name: "r").create!
    out << "where-new: #{q.name} #{q.is_not_deleted.inspect} #{made.shelf_id == shelf.id} #{made.name} #{Doc.where(name: %w[a b]).new.name.inspect} #{Doc.where.not(name: "q").new.name.inspect} #{Doc.where(is_not_deleted: false).new.is_not_deleted.inspect} #{Doc.unscoped.where(name: "u").new.is_not_deleted.inspect} #{Tag.where(status: "s").build.status} #{Doc.where(name: nil).new.name.inspect}"
    bare = Shelf.create!
    Doc.unscoped.create!(name: "x", shelf: bare, is_not_deleted: false)
    out << "left-joins: #{Shelf.left_joins(:docs).where(docs: { id: nil }).count} #{Shelf.left_outer_joins(:docs).count} #{Shelf.joins(:docs).where(id: bare.id).count}"
    Badge.create!(title: "a", status: "shown")
    Badge.create!(title: "b", status: "hidden")
    Badge.unscoped.create!(title: "c", status: "shown", archived: true)
    out << "named: #{Badge.new.archived.inspect} #{Badge.count} #{Badge.unscoped.count} #{Badge.all.map(&:title).join(",")} #{Badge.where(title: "b").exists?}"
    Rule.create!(title: "r", rule_type: 1)
    MarkdownRule.create!(title: "m")
    out << "plain: #{MarkdownRule.new.rule_type} #{MarkdownRule.count} #{Rule.count} #{MarkdownRule.all.map(&:title).join(",")} #{MarkdownRule.first.class.name} #{Rule.order(:id).last.class.name} #{MarkdownRule.unscoped.count} #{MarkdownRule.where(title: "r").exists?}"
    out
  end
end
"##;

/// A controller action's chain is the kind the arel pass folds into
/// direct SQL, whose batched preload would skip the target's default
/// scope. The emitted action must keep the preload on the relation.
/// (Here `scope_chain` roots the chain on the relation before arel sees
/// it; `default_scope::scoped_preload` is the arel pass's own guard.)
const SHELVES_CONTROLLER: &str = r#"class ShelvesController < ApplicationController
  def index
    @shelves = Shelf.includes(:docs).order(:id)
    render plain: @shelves.map { |s| s.docs.size }.join(",")
  end
end
"#;

const EXPECTED: [&str; 18] = [
    "new: true nil",
    "all: 2 a,b 0 false",
    "find: ActiveRecord::RecordNotFound nil a",
    "unscoped: 3 gone 3 2",
    "association: 2 a,b false",
    "arel: a,b",
    "belongs_to: true",
    "preload: a,b 2 2",
    "joins: 2",
    "select: a false ActiveModel::MissingAttributeError",
    "restored: 3 a,b,gone",
    "order: a,b,c a c c",
    "extending: b,c 2",
    "sti: live x,y 2 3 3 4 x",
    "where-new: q true true r nil nil false nil s nil",
    "left-joins: 1 5 0",
    "named: false 1 3 a false",
    "plain: 2 1 2 m MarkdownRule Rule 2 false",
];

/// What native Rails requires, in order.
const LOADS: [&str; 11] = [
    "app/models/application_record",
    "app/models/shelf",
    "app/models/doc",
    "app/models/tag_finders",
    "app/models/tag",
    "app/models/note",
    "app/models/pinned",
    "app/models/badge",
    "app/models/rule",
    "app/models/markdown_rule",
    "app/models/default_scope_probe",
];

const FILES: [(&str, &str); 17] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shelf.rb", SHELF),
    ("app/models/doc.rb", DOC),
    ("app/models/tag_finders.rb", TAG_FINDERS),
    ("app/models/tag.rb", TAG),
    ("app/models/note.rb", NOTE),
    ("app/models/pinned.rb", PINNED),
    ("app/models/badge.rb", BADGE),
    ("app/models/rule.rb", RULE),
    ("app/models/markdown_rule.rb", MARKDOWN_RULE),
    ("app/models/default_scope_probe.rb", PROBE),
    ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
    ("app/controllers/shelves_controller.rb", SHELVES_CONTROLLER),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn default_scope_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "default-scope",
        &FILES,
        &emitted,
        "rh_spec_default_scope",
        "DefaultScopeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
    let controller = std::fs::read_to_string(emitted.join("app/controllers/shelves_controller.rb")).expect("controller");
    assert!(controller.contains("includes(:docs)"), "the arel pass folded a default-scoped preload:\n{controller}");
}
