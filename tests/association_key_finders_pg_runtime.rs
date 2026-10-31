//! PostgreSQL: association key finders, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

use roundhouse::project::BuildTarget;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";

const ACCOUNT: &str = "class Account < ApplicationRecord\n  has_many :codes\n  has_many :tags, as: :taggable\nend\n";

const SERVICE: &str = "class Service < ApplicationRecord\n  has_many :codes\n  has_many :tags, as: :taggable\nend\n";

const CODE: &str = r##"class Code < ApplicationRecord
  belongs_to :account
  belongs_to :service, optional: true
end
"##;

const TAG: &str = "class Tag < ApplicationRecord\n  belongs_to :taggable, polymorphic: true\nend\n";

const PROBE: &str = r##"class KeyFindersProbe
  def self.lines
    out = []
    acct = Account.create!(name: "a")
    other = Account.create!(name: "b")
    s1 = Service.create!(name: "s1")
    s2 = Service.create!(name: "s2")

    c1 = acct.codes.find_or_create_by(code: "x", service: s1)
    out << "foc create: #{c1.persisted?} #{c1.service_id == s1.id} #{c1.account_id == acct.id}"
    c1b = acct.codes.find_or_create_by(code: "x", service: s1)
    out << "foc find: #{c1b.id == c1.id} #{Code.count}"
    c2 = acct.codes.find_or_create_by(code: "x", service: s2)
    out << "foc other service: #{c2.id != c1.id} #{c2.service_id == s2.id} #{Code.count}"
    cy = acct.codes.find_or_create_by(code: "y", service: s1)
    out << "foc y: #{cy.service_id == s1.id} #{Code.count}"
    cc = Code.find_or_create_by(code: "k", account: other, service: s2)
    out << "class foc: #{cc.persisted?} #{cc.account_id == other.id} #{cc.service_id == s2.id}"

    out << "where: #{Code.where(service: s1).count} #{Code.where(service: s2).count}"
    out << "where list: #{Code.where(service: [s1, s2]).count} #{Code.where(service: [s1]).count}"
    out << "where ids: #{Code.where(service: s1.id).count}"
    out << "where not: #{Code.where.not(service: s1).count}"
    out << "where on assoc: #{acct.codes.where(service: s1).map(&:code).sort.inspect}"
    out << "where two keys: #{Code.where(account: acct, service: s2).count}"
    Code.create!(code: "nosvc", account: acct)
    out << "where nil: #{Code.where(service: nil).count} #{Code.where(service_id: nil).count}"
    out << "find_by: #{Code.find_by(service: s1, code: "y").code}"
    out << "find_by assoc: #{acct.codes.find_by(service: s2).code} #{acct.codes.find_by(service: s2).id == c2.id}"
    out << "find_by none: #{acct.codes.find_by(service: s2, code: "none").inspect}"
    out << "find_by!: #{Code.find_by!(service: s1, code: "x").id == c1.id}"
    out << "exists: #{Code.exists?(service: s1)} #{acct.codes.exists?(service: s2)}"

    built = acct.codes.find_or_initialize_by(code: "z", service: s2)
    out << "foi new: #{built.new_record?} #{built.service_id == s2.id} #{built.account_id == acct.id} #{Code.count}"
    found = acct.codes.find_or_initialize_by(code: "x", service: s1)
    out << "foi found: #{found.persisted?} #{found.id == c1.id}"

    n = Code.count
    cf = acct.codes.create_or_find_by(code: "cof", service: s2)
    out << "cof: #{cf.persisted?} #{cf.service_id == s2.id} #{Code.count - n}"
    out << "cof again: #{acct.codes.create_or_find_by(code: "cof", service: s2).id == cf.id} #{Code.count - n}"

    w = Code.where(service: s1).create!(code: "viawhere", account: acct)
    out << "where create: #{w.service_id == s1.id}"
    out << "where new: #{Code.where(service: s2).new.service_id == s2.id} #{acct.codes.where(service: s2).build.service_id == s2.id}"
    cw = Code.create_with(service: s1).find_or_create_by(code: "cw", account: acct)
    out << "create_with: #{cw.persisted?} #{cw.service_id == s1.id}"
    cw2 = acct.codes.create_with(service: s2).find_or_initialize_by(code: "cw2")
    out << "create_with assoc: #{cw2.new_record?} #{cw2.service_id == s2.id} #{cw2.account_id == acct.id}"

    t1 = Tag.find_or_create_by(taggable: acct, label: "t")
    out << "poly foc: #{t1.persisted?} #{t1.taggable_id == acct.id} #{t1.taggable_type}"
    t1b = Tag.find_or_create_by(taggable: acct, label: "t")
    out << "poly foc find: #{t1b.id == t1.id} #{Tag.count}"
    Tag.create!(taggable: other, label: "o")
    Tag.create!(taggable_id: acct.id, taggable_type: "Service", label: "s")
    out << "poly where: #{Tag.where(taggable: acct).count} #{Tag.where(taggable: other).count} #{Tag.where(taggable: s1).count}"
    out << "poly where nil: #{Tag.where(taggable: nil).count}"
    out << "poly find_by: #{Tag.find_by(taggable: other).label} #{acct.tags.find_by(taggable: acct).label}"
    out << "poly new: #{Tag.where(taggable: other).new.taggable_type}"

    owner = Account.create!(name: "p")
    base = Code.count
    r = owner.public_send(:codes) << Code.new(code: "p1")
    out << "send push: #{Code.count - base} #{r.size} #{owner.codes.map(&:persisted?).inspect} #{owner.codes.last.account_id == owner.id}"
    list = owner.codes
    list << Code.new(code: "p2")
    out << "local push: #{Code.count - base} #{list.size} #{Code.where(account_id: owner.id).count}"
    list.push(Code.new(code: "p3"), Code.new(code: "p4"))
    out << "local push many: #{Code.count - base} #{list.size}"
    list.concat([Code.new(code: "p5")])
    out << "local concat: #{Code.count - base} #{list.size} #{Account.find(owner.id).codes.count}"
    fresh = Account.find(owner.id)
    fresh.public_send(:codes) << Code.new(code: "p6")
    out << "unloaded send push: #{Code.count - base} #{Account.find(owner.id).codes.count}"
    newbie = Account.new(name: "n")
    newbie.public_send(:codes) << Code.new(code: "n1")
    out << "new owner send push: #{Code.count - base} #{newbie.codes.size}"
    newbie.save!
    out << "new owner saved: #{Code.count - base} #{newbie.codes.map(&:persisted?).inspect}"
    holder = Account.create!(name: "h")
    held = Code.count
    holder.codes.to_a
    holder.association(:codes).target << Code.new(code: "h1")
    out << "target push: #{Code.count - held} #{holder.codes.size} #{holder.codes.map(&:persisted?).inspect}"
    holder.codes.target << Code.new(code: "h2")
    out << "proxy target push: #{Code.count - held} #{holder.codes.size}"
    holder.save!
    out << "target saved with owner: #{Code.count - held} #{holder.codes.map(&:persisted?).inspect}"
    out
  end
end
"##;

const SCHEMA_STRUCTURE: &str = r#"CREATE TABLE accounts (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
CREATE TABLE services (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT services_pkey PRIMARY KEY (id)
);
CREATE TABLE codes (
    id bigserial NOT NULL,
    code character varying,
    account_id bigint,
    service_id bigint,
    CONSTRAINT codes_pkey PRIMARY KEY (id),
    CONSTRAINT codes_unique UNIQUE (account_id, code, service_id)
);
CREATE TABLE tags (
    id bigserial NOT NULL,
    label character varying,
    taggable_id bigint,
    taggable_type character varying,
    CONSTRAINT tags_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str =
    "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_association_key_finders\n";

const FILES: &[(&str, &str)] = &[
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/account.rb", ACCOUNT),
    ("app/models/service.rb", SERVICE),
    ("app/models/code.rb", CODE),
    ("app/models/tag.rb", TAG),
    ("app/models/key_finders_probe.rb", PROBE),
];

/// Native Rails on the same PostgreSQL, so two identical wrong answers
/// still fail.
const EXPECTED: &[&str] = &[
    "foc create: true true true",
    "foc find: true 1",
    "foc other service: true true 2",
    "foc y: true 3",
    "class foc: true true true",
    "where: 2 2",
    "where list: 4 2",
    "where ids: 2",
    "where not: 2",
    "where on assoc: [\"x\", \"y\"]",
    "where two keys: 1",
    "where nil: 1 1",
    "find_by: y",
    "find_by assoc: x true",
    "find_by none: nil",
    "find_by!: true",
    "exists: true true",
    "foi new: true true true 5",
    "foi found: true true",
    "cof: true true 1",
    "cof again: true 1",
    "where create: true",
    "where new: true true",
    "create_with: true true",
    "create_with assoc: true true true",
    "poly foc: true true Account",
    "poly foc find: true 1",
    "poly where: 1 1 1",
    "poly where nil: 0",
    "poly find_by: o t",
    "poly new: Account",
    "send push: 1 1 [true] true",
    "local push: 2 2 2",
    "local push many: 4 4",
    "local concat: 5 5 5",
    "unloaded send push: 6 6",
    "new owner send push: 6 1",
    "new owner saved: 7 [true]",
    "target push: 0 1 [false]",
    "proxy target push: 0 2",
    "target saved with owner: 2 [true, true]",
];

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn association_key_finders_like_activerecord_on_postgresql() {
    let mut files = FILES.to_vec();
    files.extend([
        ("roundhouse.yml", ""),
        ("config/database.yml", DATABASE_YML),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", SCHEMA_STRUCTURE),
    ]);
    let (emitted, errors) =
        files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text)).emit(BuildTarget::Ruby);
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    let loads: Vec<&str> = FILES.iter().map(|(path, _)| path.trim_end_matches(".rb")).collect();
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "association-key-finders",
        &files,
        &emitted,
        "rh_spec_association_key_finders",
        "KeyFindersProbe",
        &loads,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
