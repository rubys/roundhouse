//! PostgreSQL: association scope inverse, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE shops (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
CREATE TABLE items (
    id bigserial NOT NULL,
    shop_id bigint,
    title character varying,
    role character varying,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
CREATE TABLE notes (
    id bigserial NOT NULL,
    item_id bigint,
    body character varying,
    CONSTRAINT notes_pkey PRIMARY KEY (id)
);
CREATE TABLE tags (
    id bigserial NOT NULL,
    taggable_id bigint,
    taggable_type character varying,
    label character varying,
    CONSTRAINT tags_pkey PRIMARY KEY (id)
);
CREATE TABLE profiles (
    id bigserial NOT NULL,
    shop_id bigint,
    CONSTRAINT profiles_pkey PRIMARY KEY (id)
);
CREATE TABLE badges (
    id bigserial NOT NULL,
    holder_id bigint,
    CONSTRAINT badges_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_association_scope
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :items, -> { order("title DESC") }
  has_many :tags, as: :taggable
  has_one :profile
  has_one :main_item, -> { where(role: "main") }, class_name: "Item"
  has_many :notes, through: :items
  has_one :badge, inverse_of: :holder, foreign_key: :holder_id
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
  belongs_to :shop, optional: true
  has_many :notes
end
"#;

const NOTE: &str = r#"class Note < ApplicationRecord
  belongs_to :item
end
"#;

const TAG: &str = r#"class Tag < ApplicationRecord
  belongs_to :taggable, polymorphic: true
end
"#;

const PROFILE: &str = r#"class Profile < ApplicationRecord
  belongs_to :shop
end
"#;

const BADGE: &str = r#"class Badge < ApplicationRecord
  belongs_to :holder, class_name: "Shop", foreign_key: :holder_id, inverse_of: :badge
end
"#;

const PROBE: &str = r##"class ScopeProbe
  def self.lines
    out = []
    shop = Shop.create!(name: "s")
    Item.create!(shop_id: shop.id, title: "a", role: "main")
    b = Item.create!(shop_id: shop.id, title: "b", role: "x")
    Item.create!(title: "other", role: "main")
    Note.create!(item_id: b.id, body: "n1")
    Tag.create!(taggable: shop, label: "t1")
    Tag.create!(taggable_type: "Item", taggable_id: shop.id, label: "t2")
    owner = Shop.find(shop.id)

    out << "scoped has_many: #{owner.association(:items).scope.map(&:title).inspect}"
    out << "scoped has_many klass: #{owner.association(:items).scope.klass}"
    out << "scoped has_many order: #{owner.association(:items).scope.to_sql.include?("title DESC")}"
    out << "as: #{owner.association(:tags).scope.map(&:label).inspect}"
    out << "has_one scoped: #{owner.association(:main_item).scope.map(&:title).inspect}"
    out << "through: #{owner.association(:notes).scope.map(&:body).inspect}"
    out << "new owner: #{Shop.new.association(:items).scope.to_a.inspect}"
    out << "scope loads nothing: #{owner.association(:items).loaded?}"

    profile = Profile.create!(shop_id: shop.id)
    held = Profile.find(profile.id)
    holder = Shop.find(shop.id)
    association = held.association(:shop)
    association.target = holder
    out << "inverse before: #{holder.association(:profile).loaded?}"
    out << "set_inverse_instance: #{association.set_inverse_instance(holder).equal?(holder)}"
    out << "inverse after: #{holder.association(:profile).loaded?} #{holder.profile.equal?(held)}"

    badge = Badge.create!(holder_id: shop.id)
    badge_held = Badge.find(badge.id)
    badge_owner = Shop.find(shop.id)
    badge_held.association(:holder).set_inverse_instance(badge_owner)
    out << "explicit inverse_of: #{badge_owner.association(:badge).loaded?} #{badge_owner.badge.equal?(badge_held)}"

    # prefetched target through them).
    fetched = Profile.find(profile.id)
    profile_owner = Shop.find(shop.id)
    profile_owner.association(:profile).target = fetched
    out << "has_one set_inverse_instance: #{profile_owner.association(:profile).set_inverse_instance(fetched).equal?(fetched)} #{fetched.association(:shop).loaded?} #{fetched.shop.equal?(profile_owner)}"
    note = Note.find_by!(body: "n1")
    note_owner = Item.find(b.id)
    note_owner.association(:notes).target = [note]
    out << "has_many set_inverse_instance: #{note_owner.association(:notes).set_inverse_instance(note).equal?(note)} #{note.association(:item).loaded?} #{note.item.equal?(note_owner)}"

    nobody = Item.find(b.id)
    nobody.association(:shop).set_inverse_instance(owner)
    out << "no inverse: #{owner.association(:items).loaded?}"
    out
  end
end
"##;

const EXPECTED: [&str; 15] = [
    r#"scoped has_many: ["b", "a"]"#,
    "scoped has_many klass: Item",
    "scoped has_many order: true",
    r#"as: ["t1"]"#,
    r#"has_one scoped: ["a"]"#,
    r#"through: ["n1"]"#,
    "new owner: []",
    "scope loads nothing: false",
    "inverse before: false",
    "set_inverse_instance: true",
    "inverse after: true true",
    "explicit inverse_of: true true",
    "has_one set_inverse_instance: true true true",
    "has_many set_inverse_instance: true true true",
    "no inverse: false",
];

/// What native Rails requires, in order.
const LOADS: [&str; 8] = [
    "app/models/application_record",
    "app/models/shop",
    "app/models/item",
    "app/models/note",
    "app/models/tag",
    "app/models/profile",
    "app/models/badge",
    "app/models/scope_probe",
];

const FILES: [(&str, &str); 12] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/item.rb", ITEM),
    ("app/models/note.rb", NOTE),
    ("app/models/tag.rb", TAG),
    ("app/models/profile.rb", PROFILE),
    ("app/models/badge.rb", BADGE),
    ("app/models/scope_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: the reader's query sits beside it for a scoped
/// has_many, a has_one and an `as:` has_many, and a belongs_to's
/// association object is given its inverse's name.
#[test]
#[ignore = "fork spec: postgresql"]
fn association_scope_and_inverse_are_emitted_for_every_kind() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap();
    let shop = read("app/models/shop.rb");
    for expected in [
        "def _items_association_scope",
        "def _tags_association_scope",
        "def _main_item_association_scope",
    ] {
        assert!(shop.contains(expected), "missing {expected:?} in:\n{shop}");
    }
    let profile = read("app/models/profile.rb");
    assert!(
        profile.contains("BelongsToAssociation.new(self, :shop, \"Shop\", :shop_id, true, :id, :profile)"),
        "automatic inverse missing in:\n{profile}"
    );
    let badge = read("app/models/badge.rb");
    assert!(
        badge.contains("BelongsToAssociation.new(self, :holder, \"Shop\", :holder_id, true, :id, :badge)"),
        "inverse_of: missing in:\n{badge}"
    );
    let item = read("app/models/item.rb");
    assert!(
        item.contains("BelongsToAssociation.new(self, :shop, \"Shop\", :shop_id, true, :id, nil)"),
        "a scoped has_many is no automatic inverse:\n{item}"
    );
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn association_scope_and_set_inverse_instance_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "association-scope",
        &FILES,
        &emitted,
        "rh_spec_association_scope",
        "ScopeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
