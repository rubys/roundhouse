//! PostgreSQL: association persistence, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

use roundhouse::project::BuildTarget;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const SHOP: &str = r##"class Shop < ApplicationRecord
  has_many :entries, dependent: :destroy
  has_many :notes
  has_one :profile
end
"##;
const ENTRY: &str = r##"class Entry < ApplicationRecord
  belongs_to :shop

  # Another model's `state` enum, so a label on a relation the compiler
  # cannot attribute to a model does not name one mapping.
  enum :state, [:closed, :open, :shut, :active], default: :closed, suffix: true

  MARKS = [] #: Array[String]

  #: -> Array[String]
  def self.marks = MARKS

  def destroy
    reflection = destroyed_by_association
    Entry.marks << (reflection.nil? ? "direct" : reflection.name.to_s)
    super
  end

  def pending_id?
    id_changed?
  end
end
"##;
const NOTE: &str = r##"class Note < ApplicationRecord
  TAGS = [] #: Array[String]

  #: -> Array[String]
  def self.tags = TAGS

  # `define_method`, whose `super` is the association's own reader.
  #: (Array[Symbol]) -> void
  def self.tag_readers(names)
    names.each do |name|
      unsafe_self = self #: as untyped
      unsafe_self.define_method(name) do
        TAGS << name.to_s
        super()
      end
    end
  end

  tag_readers([:shop])

  belongs_to :shop, optional: true

  enum :state, [:draft, :active], default: :draft, suffix: true
end
"##;
const PROFILE: &str = "class Profile < ApplicationRecord\n  belongs_to :shop, optional: true\nend\n";
const PROBE: &str = r##"class AfterCommitDuck
  #: { -> void } -> void
  def initialize(&block)
    @callback = block
  end

  #: (?Hash[untyped, untyped] _args) -> void
  def committed!(_args = {})
    @callback.call
  end

  #: -> void
  def before_committed!; end

  #: (?Hash[untyped, untyped] _args) -> void
  def rolledback!(_args = {}); end

  #: -> bool
  def trigger_transactional_callbacks?
    false
  end
end

class PersistenceProbe
  # A bare Relation type: the compiler cannot see the model, so an enum
  # label in a condition on it is mapped at run time.
  #: (Integer) -> ActiveRecord::Relation
  def self.note_scope(shop_id)
    model = Object.const_get("Note") #: as untyped
    model.where(shop_id: shop_id)
  end

  def self.lines
    out = []
    shop = Shop.create!(name: "one")
    Shop.create!(name: "two")
    a = Entry.create!(shop: shop, title: "a")
    b = Entry.create!(shop: shop, title: "b")
    c = Entry.create!(shop: shop, title: "c")
    Entry.create!(shop: shop, title: "d")
    shop = Shop.find(shop.id)
    entries = shop.entries #: as untyped
    gone = entries.destroy(entries.first)
    out << "destroy record: #{gone.map(&:title).inspect} held #{shop.entries.map(&:title).inspect} rows #{Entry.where(shop_id: shop.id).order(:id).pluck(:title).inspect}"
    gone = shop.entries.destroy(c.id)
    out << "destroy id: #{gone.map(&:title).inspect} held #{shop.entries.map(&:title).inspect} rows #{Entry.where(shop_id: shop.id).order(:id).pluck(:title).inspect}"
    out << "marks: #{Entry.marks.inspect}"

    n1 = Note.create!(shop: shop, title: "n1")
    n2 = Note.create!(shop: shop, title: "n2")
    shop = Shop.find(shop.id)
    notes = shop.notes #: as untyped
    notes.delete(notes.first)
    out << "delete nullifies: held #{shop.notes.map(&:title).inspect} orphan shop_id #{Note.find(n1.id).shop_id.inspect} total #{Note.count}"
    fresh = Note.new(title: "n3")
    shop.notes = [n2, fresh]
    out << "writer keeps and adds: #{shop.notes.map(&:title).inspect} rows #{Note.where(shop_id: shop.id).order(:id).pluck(:title).inspect} saved #{fresh.persisted?}"
    shop.notes = []
    out << "writer empties: #{shop.notes.size} rows #{Note.where(shop_id: shop.id).count}"

    shop.profile = Profile.new(bio: "x")
    out << "has_one writer saves: #{Profile.where(shop_id: shop.id).pluck(:bio).inspect}"
    shop.profile = nil
    out << "has_one writer nullifies: #{Profile.where(shop_id: shop.id).count} total #{Profile.count}"

    Entry.marks.clear
    shop.destroy
    out << "cascade marks: #{Entry.marks.inspect} left #{Entry.count}"

    ran = [] #: Array[String]
    ActiveRecord::Base.transaction do
      ActiveRecord::Base.connection.add_transaction_record(AfterCommitDuck.new { ran << "after commit" }) #: as untyped
      ran << "in transaction"
    end
    out << "duck record: #{ran.inspect}"

    out << "id_changed? new #{Entry.new.pending_id?} saved #{b.pending_id?}"

    s = Shop.create!(name: "three")
    kept = Note.create!(shop: s, title: "kept")
    owner = Note.find(kept.id).shop #: as untyped
    out << "reader super: #{owner.nil? ? "nil" : owner.name} #{Note.tags.inspect}"
    Entry.create!(shop: s, title: "p")
    Entry.create!(shop: s, title: "q")
    rows = Entry.where(shop_id: s.id).select(:title).select(:id).order(:id).map { |e| [e.title, e.id.class.name] }
    Note.create!(shop: s, title: "live", state: :active)
    scope = note_scope(s.id)
    live = scope.where(state: :active).or(scope.where(id: ..0)).map(&:title)
    out << "enum through or: #{live.inspect}"
    listed = scope.where(state: [:active, :draft]).or(scope.where(id: ..0)).order(:id).map(&:title)
    out << "enum array: #{listed.inspect} not #{scope.where.not(state: [:draft]).map(&:title).inspect}"
    out << "select adds: #{rows.inspect}"
    first = Shop.joins("INNER JOIN entries ON entries.shop_id = shops.id").order(:id).first
    out << "ordered over a join: #{first.nil? ? "nil" : first.name}"
    Shop.create!(name: "four")
    page = Shop.eager_load(:entries).order(:id).limit(2).to_a #: as untyped
    out << "limited eager_load: #{page.map { |shop2| [shop2.name, shop2.entries.size] }.inspect}"
    out
  end
end
"##;
const EXPECTED: &[&str] = &[
    "destroy record: [\"a\"] held [\"b\", \"c\", \"d\"] rows [\"b\", \"c\", \"d\"]",
    "destroy id: [\"c\"] held [\"b\", \"d\"] rows [\"b\", \"d\"]",
    "marks: [\"direct\", \"direct\"]",
    "delete nullifies: held [\"n2\"] orphan shop_id nil total 2",
    "writer keeps and adds: [\"n2\", \"n3\"] rows [\"n2\", \"n3\"] saved true",
    "writer empties: 0 rows 0",
    "has_one writer saves: [\"x\"]",
    "has_one writer nullifies: 0 total 1",
    "cascade marks: [\"entries\", \"entries\"] left 0",
    "duck record: [\"in transaction\", \"after commit\"]",
    "id_changed? new false saved false",
    "reader super: three [\"shop\"]",
    "enum through or: [\"live\"]",
    "enum array: [\"kept\", \"live\"] not [\"live\"]",
    "select adds: [[\"p\", \"Integer\"], [\"q\", \"Integer\"]]",
    "ordered over a join: three",
    "limited eager_load: [[\"two\", 0], [\"three\", 2]]",
];
fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/shop.rb", SHOP),
        ("app/models/entry.rb", ENTRY),
        ("app/models/note.rb", NOTE),
        ("app/models/profile.rb", PROFILE),
        ("app/models/persistence_probe.rb", PROBE),
    ]
}

const STRUCTURE: &str = r#"CREATE TABLE shops (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
CREATE TABLE entries (
    id bigserial NOT NULL,
    shop_id bigint,
    title character varying,
    state integer DEFAULT 0 NOT NULL,
    CONSTRAINT entries_pkey PRIMARY KEY (id)
);
CREATE TABLE notes (
    id bigserial NOT NULL,
    shop_id bigint,
    title character varying,
    state integer DEFAULT 0 NOT NULL,
    CONSTRAINT notes_pkey PRIMARY KEY (id)
);
CREATE TABLE profiles (
    id bigserial NOT NULL,
    shop_id bigint,
    bio character varying,
    CONSTRAINT profiles_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str =
    "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_assoc_persistence\n";

#[test]
#[ignore = "fork spec: postgresql"]
fn a_has_many_reader_persists_through_destroy_delete_and_the_writers_as_native() {
    let mut files = files();
    files.extend([
        ("roundhouse.yml", ""),
        ("config/database.yml", DATABASE_YML),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", STRUCTURE),
    ]);
    let (emitted, errors) =
        files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text)).emit(BuildTarget::Ruby);
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    let loads = [
        "app/models/application_record",
        "app/models/shop",
        "app/models/entry",
        "app/models/note",
        "app/models/profile",
        "app/models/persistence_probe",
    ];
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "assoc-persistence",
        &files,
        &emitted,
        "rh_spec_assoc_persistence",
        "PersistenceProbe",
        &loads,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
