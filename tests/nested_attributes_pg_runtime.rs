//! PostgreSQL: nested attributes, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE groups (
    id bigserial NOT NULL,
    title character varying,
    CONSTRAINT groups_pkey PRIMARY KEY (id)
);
CREATE TABLE orders (
    id bigserial NOT NULL,
    name character varying,
    group_id bigint,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);
CREATE TABLE lines (
    id bigserial NOT NULL,
    order_id bigint,
    sku character varying,
    qty integer DEFAULT '1' NOT NULL,
    CONSTRAINT lines_pkey PRIMARY KEY (id)
);
CREATE TABLE badges (
    id bigserial NOT NULL,
    order_id bigint,
    label character varying,
    CONSTRAINT badges_pkey PRIMARY KEY (id)
);
CREATE TABLE profiles (
    id bigserial NOT NULL,
    order_id bigint,
    bio character varying,
    CONSTRAINT profiles_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_nested_attributes
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";

const ORDER: &str = r##"class Order < ApplicationRecord
  has_many :lines
  has_one :profile
  has_one :badge
  belongs_to :group, optional: true
  validates :name, presence: true

  accepts_nested_attributes_for :lines, allow_destroy: true, reject_if: :all_blank, limit: 3
  accepts_nested_attributes_for :profile, update_only: true
  accepts_nested_attributes_for :badge, allow_destroy: true
  accepts_nested_attributes_for :group, allow_destroy: true, reject_if: ->(attrs) { attrs[:title].blank? }
end
"##;

const LINE: &str = r##"class Line < ApplicationRecord
  belongs_to :order
  validates :sku, presence: true
end
"##;

const PROFILE: &str = r##"class Profile < ApplicationRecord
  belongs_to :order
  validates :bio, presence: true
end
"##;

const BADGE: &str = r##"class Badge < ApplicationRecord
  belongs_to :order, optional: true
end
"##;

const GROUP: &str = r##"class Group < ApplicationRecord
  has_many :orders
  validates :title, presence: true
end
"##;

const PROBE: &str = r##"class NestedAttributesProbe
  def self.skus(order)
    Line.where(order_id: order.id).order(:id).pluck(:sku).inspect
  end

  def self.lines
    out = []

    o = Order.new(name: "a", lines_attributes: [{ sku: "x" }, { sku: "y", qty: 2 }, { sku: "", qty: "" }])
    out << "built: #{o.lines.map(&:sku).inspect} #{o.lines.map(&:new_record?).inspect}"
    out << "saved: #{o.save} #{skus(o)} #{o.lines.map(&:order_id).uniq == [o.id]}"

    o = Order.find(o.id)
    first, second = Line.where(order_id: o.id).order(:id).to_a
    out << "update: #{o.update(lines_attributes: [{ id: first.id, sku: "x2" }, { id: second.id.to_s, qty: 5 }])} #{skus(o)} #{Line.find(second.id).qty}"

    o = Order.find(o.id)
    o.lines_attributes = [{ id: first.id, _destroy: "1" }]
    out << "marked: #{o.lines.map(&:marked_for_destruction?).inspect} #{o.lines.first.changed_for_autosave?} #{skus(o)}"
    out << "destroyed: #{o.save} #{skus(o)} #{o.lines.size}"

    o = Order.find(o.id)
    o.assign_attributes(lines_attributes: { "0" => { sku: "h1" }, "1" => { sku: "h2" } })
    out << "hash form: #{o.lines.map(&:sku).inspect} #{o.save} #{skus(o)}"

    o = Order.find(o.id)
    o.assign_attributes(lines_attributes: [{ sku: "" , qty: "" }, { _destroy: "1", sku: "gone" }])
    out << "rejected: #{o.lines.size} #{o.save} #{skus(o)}"

    begin
      Order.new(name: "b", lines_attributes: [{ sku: "1" }, { sku: "2" }, { sku: "3" }, { sku: "4" }])
    rescue ActiveRecord::NestedAttributes::TooManyRecords => e
      out << "limit: #{e.class} #{e.message}"
    end

    o = Order.find(o.id)
    begin
      o.update(lines_attributes: [{ id: 0, sku: "nope" }])
    rescue ActiveRecord::RecordNotFound => e
      out << "missing: #{e.message} #{e.model} #{e.id}"
    end

    begin
      o.lines_attributes = "bad"
    rescue ArgumentError => e
      out << "bad type: #{e.message}"
    end

    bad = Order.new(name: "c", lines_attributes: [{ sku: "ok" }, { qty: 3 }])
    out << "invalid: #{bad.save} #{bad.errors.full_messages.inspect} #{Order.where(name: "c").count}"

    o = Order.find(o.id)
    existing = Line.where(order_id: o.id).first
    o.lines_attributes = [{ id: existing.id, sku: "" }]
    out << "invalid update: #{o.save} #{o.errors.full_messages.inspect} #{Line.find(existing.id).sku}"

    p1 = Order.create!(name: "d", profile_attributes: { bio: "first" })
    out << "has_one build: #{p1.profile.bio} #{Profile.where(order_id: p1.id).pluck(:bio).inspect}"
    p1 = Order.find(p1.id)
    p1.update(profile_attributes: { bio: "second" })
    out << "has_one update_only: #{Profile.where(order_id: p1.id).pluck(:bio).inspect}"
    p1.update(profile_attributes: { bio: "third", _destroy: "1" })
    out << "has_one no allow_destroy: #{Profile.where(order_id: p1.id).pluck(:bio).inspect}"
    p1.update(profile_attributes: { bio: "" })
    out << "has_one invalid: #{p1.errors.full_messages.inspect} #{Profile.where(order_id: p1.id).pluck(:bio).inspect}"

    g = Order.create!(name: "e", group_attributes: { title: "t1" })
    out << "belongs_to build: #{g.group.title} #{Group.where(id: g.group_id).pluck(:title).inspect}"
    g2 = Order.create!(name: "f", group_attributes: { title: "" })
    out << "belongs_to rejected: #{g2.group.inspect} #{g2.group_id.inspect}"
    g3 = Order.find(g.id)
    g3.update(group_attributes: { id: g.group_id, title: "t2" })
    out << "belongs_to update: #{Group.find(g.group_id).title}"

    g3 = Order.find(g.id)
    g3.update(group_attributes: { id: g.group_id, _destroy: "1" })
    out << "belongs_to destroy: #{g3.group_id.inspect} #{Group.where(id: g.group_id).count} #{Order.find(g.id).group_id.inspect}"

    b = Order.create!(name: "g", badge_attributes: { label: "gold" })
    b = Order.find(b.id)
    out << "has_one mismatch: #{(b.update(badge_attributes: { id: 0, label: "x" }) rescue $!.message)}"
    b.update(badge_attributes: { id: b.badge.id, label: "silver" })
    out << "has_one id update: #{Badge.where(order_id: b.id).pluck(:label).inspect}"
    b.update(badge_attributes: { label: "bronze" })
    out << "has_one replace: #{Badge.where(order_id: b.id).order(:id).pluck(:label).inspect}"
    b = Order.find(b.id)
    b.update(badge_attributes: { id: b.badge.id, _destroy: "1" })
    out << "has_one destroy: #{Badge.where(order_id: b.id).count}"

    out << "options: #{Order.nested_attributes_options.keys.inspect} #{Order.nested_attributes_options[:lines][:limit]} #{Order.nested_attributes_options[:profile][:update_only]}"
    begin
      Order.accepts_nested_attributes_for :nope
    rescue ArgumentError => e
      out << "unknown association: #{e.message}"
    end
    begin
      Order.accepts_nested_attributes_for :lines, bogus: true
    rescue ArgumentError => e
      out << "unknown option: #{e.message}"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 27] = [
    r#"built: ["x", "y"] [true, true]"#,
    r#"saved: true ["x", "y"] true"#,
    r#"update: true ["x2", "y"] 5"#,
    r#"marked: [true, false] true ["x2", "y"]"#,
    r#"destroyed: true ["y"] 1"#,
    r#"hash form: ["y", "h1", "h2"] true ["y", "h1", "h2"]"#,
    r#"rejected: 3 true ["y", "h1", "h2"]"#,
    r#"limit: ActiveRecord::NestedAttributes::TooManyRecords Maximum 3 records are allowed. Got 4 records instead."#,
    r#"missing: Couldn't find Line with ID=0 for Order with ID=1 Line 0"#,
    r#"bad type: Hash or Array expected for `lines` attributes, got String"#,
    r#"invalid: false ["Lines sku can't be blank"] 0"#,
    r#"invalid update: false ["Lines sku can't be blank"] y"#,
    r#"has_one build: first ["first"]"#,
    r#"has_one update_only: ["second"]"#,
    r#"has_one no allow_destroy: ["third"]"#,
    r#"has_one invalid: ["Profile bio can't be blank"] ["third"]"#,
    r#"belongs_to build: t1 ["t1"]"#,
    r#"belongs_to rejected: nil nil"#,
    r#"belongs_to update: t2"#,
    r#"belongs_to destroy: nil 0 nil"#,
    r#"has_one mismatch: Couldn't find Badge with ID=0 for Order with ID=5"#,
    r#"has_one id update: ["silver"]"#,
    r#"has_one replace: ["bronze"]"#,
    r#"has_one destroy: 0"#,
    r#"options: [:lines, :profile, :badge, :group] 3 true"#,
    r#"unknown association: No association found for name `nope'. Has it been defined yet?"#,
    r#"unknown option: Unknown key: :bogus. Valid keys are: :allow_destroy, :reject_if, :limit, :update_only"#,
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/order.rb", ORDER),
    ("app/models/line.rb", LINE),
    ("app/models/profile.rb", PROFILE),
    ("app/models/group.rb", GROUP),
    ("app/models/badge.rb", BADGE),
    ("app/models/nested_attributes_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn nested_attributes_write_and_autosave_as_natively_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "nested-attributes",
        &FILES,
        &emitted,
        "rh_spec_nested_attributes",
        "NestedAttributesProbe",
        &[
            "app/models/application_record",
            "app/models/order",
            "app/models/line",
            "app/models/profile",
            "app/models/group",
            "app/models/badge",
            "app/models/nested_attributes_probe",
        ],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The compiler side: the macro is replayed in the class body, after the
/// methods it reads, and the models carry the association object and
/// the reflections the writers use.
#[test]
#[ignore = "fork spec: postgresql"]
fn macro_is_replayed_in_the_class_body() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let order = std::fs::read_to_string(emitted.join("app/models/order.rb")).expect("order.rb");
    for expected in [
        "accepts_nested_attributes_for :lines, allow_destroy: true, reject_if: :all_blank, limit: 3",
        "accepts_nested_attributes_for :profile, update_only: true",
        "def self.reflect_on_association(association)",
        "def association(name)",
        "def build_profile(attributes = {}, &block)",
    ] {
        assert!(order.contains(expected), "missing {expected:?} in:\n{order}");
    }
    let at = |needle: &str| order.find(needle).unwrap_or_else(|| panic!("{needle} in:\n{order}"));
    assert!(at("def self.reflect_on_association") < at("accepts_nested_attributes_for :lines"), "{order}");
    let runtime = std::fs::read_to_string(emitted.join("runtime/active_record_nested_attributes.rb")).expect("runtime");
    assert!(runtime.contains("def accepts_nested_attributes_for"), "{runtime}");
}
