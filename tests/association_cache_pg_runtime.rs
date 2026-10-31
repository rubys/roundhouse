//! PostgreSQL: association cache, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

use roundhouse::project::BuildTarget;
use std::process::Command;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const SHOP: &str = "class Shop < ApplicationRecord\n  has_many :products\n  has_one :profile\nend\n";
const PRODUCT: &str = "class Product < ApplicationRecord\n  belongs_to :shop\nend\n";
const PROFILE: &str = "class Profile < ApplicationRecord\n  belongs_to :shop\nend\n";

/// Reads the association objects too (`loaded?`, `reset`,
/// `stale_target?`).
const PROBE: &str = r##"class CacheProbe
  def self.lines
    out = []
    fresh = Product.new(shop: Shop.new(name: "unsaved"), title: "f")
    out << "unsaved owner, new target: #{fresh.shop.name} #{fresh.shop.equal?(fresh.shop)} #{fresh.shop.new_record?}"
    s = Shop.create!(name: "one")
    s2 = Shop.create!(name: "two")
    held = Product.new(shop: s, title: "h")
    out << "unsaved owner, saved target: #{held.shop.equal?(s)} #{held.shop_id == s.id}"
    keyed = Product.new(shop_id: s.id, title: "k")
    first = keyed.shop
    out << "unsaved owner by key: #{first.name} #{keyed.shop.equal?(first)}"
    held.save!
    p = Product.find(held.id)
    a = p.shop
    a.name = "changed"
    out << "belongs_to cached: #{p.shop.name} #{p.shop.equal?(a)} #{p.association(:shop).loaded?}"
    Shop.where(id: s.id).update_all(name: "db")
    out << "belongs_to stale row: #{p.shop.name}"
    p.reload
    out << "after reload: #{p.association(:shop).loaded?} #{p.shop.name} #{p.shop.equal?(a)}"
    b = p.shop
    p.shop_id = s2.id
    out << "fk change: #{p.association(:shop).loaded?} #{p.shop.name} #{p.shop.equal?(b)}"
    p.shop_id = s.id
    c = p.shop
    Shop.where(id: s.id).update_all(name: "db2")
    p.association(:shop).reset
    out << "reset: #{p.association(:shop).loaded?} #{p.shop.name} #{p.shop.equal?(c)}"
    d = p.shop
    d.name = "in memory"
    p.shop_id = s.id
    out << "same key: #{p.shop.name} #{p.shop.equal?(d)}"
    p.shop = s2
    out << "writer: #{p.shop.name} #{p.shop_id == s2.id} #{p.shop.equal?(s2)}"
    p.shop_id = s.id
    out << "stale: #{p.association(:shop).loaded?} #{p.association(:shop).stale_target?}"
    p.reload
    out << "stale then reload: #{p.shop.name} #{p.association(:shop).loaded?} #{p.association(:shop).stale_target?}"
    Product.create!(shop: s, title: "two")
    shop = Shop.find(s.id)
    out << "has_many before read: #{shop.products.loaded?}"
    items = shop.products.to_a
    items.first.title = "edited"
    out << "has_many cached: #{shop.products.loaded?} #{shop.products.map(&:title).sort.inspect}"
    Product.create!(shop: s, title: "outside")
    out << "has_many stale: #{shop.products.size} #{shop.products.map(&:title).sort.inspect}"
    shop.products.create!(title: "inside")
    out << "has_many create: #{shop.products.map(&:title).sort.inspect}"
    shop.reload
    out << "has_many reload: #{shop.products.loaded?} #{shop.products.map(&:title).sort.inspect}"
    shop.association(:products).reset
    out << "has_many reset: #{shop.products.loaded?}"
    Profile.create!(shop: s, bio: "b")
    pr = shop.profile
    pr.bio = "edited"
    out << "has_one cached: #{shop.profile.bio} #{shop.profile.equal?(pr)}"
    Profile.where(shop_id: s.id).update_all(bio: "db")
    out << "has_one stale: #{shop.profile.bio}"
    shop.reload
    out << "has_one reload: #{shop.profile.bio}"
    out
  end
end
"##;

/// What native Rails answers (checked on SQLite and the task
/// PostgreSQL), so two identical wrong answers still fail.
const EXPECTED: &[&str] = &[
    "unsaved owner, new target: unsaved true true",
    "unsaved owner, saved target: true true",
    "unsaved owner by key: one true",
    "belongs_to cached: changed true true",
    "belongs_to stale row: changed",
    "after reload: false db false",
    "fk change: true two false",
    "reset: false db2 false",
    "same key: in memory true",
    "writer: two true true",
    "stale: true true",
    "stale then reload: db2 true false",
    "has_many before read: false",
    "has_many cached: true [\"edited\", \"two\"]",
    "has_many stale: 2 [\"edited\", \"two\"]",
    "has_many create: [\"edited\", \"inside\", \"two\"]",
    "has_many reload: false [\"h\", \"inside\", \"outside\", \"two\"]",
    "has_many reset: false",
    "has_one cached: edited true",
    "has_one stale: edited",
    "has_one reload: db",
];

/// The same reads in an app that never names the association object
/// or `loaded?`: the readers cache there too.
const PLAIN_PROBE: &str = r##"class CacheProbe
  def self.lines
    out = []
    fresh = Product.new(shop: Shop.new(name: "unsaved"), title: "f")
    out << "unsaved owner: #{fresh.shop.name} #{fresh.shop.equal?(fresh.shop)}"
    s = Shop.create!(name: "one")
    s2 = Shop.create!(name: "two")
    p = Product.create!(shop: s, title: "p")
    p = Product.find(p.id)
    a = p.shop
    a.name = "changed"
    Shop.where(id: s.id).update_all(name: "db")
    out << "belongs_to cached: #{p.shop.name} #{p.shop.equal?(a)}"
    p.shop_id = s.id
    out << "same key: #{p.shop.name}"
    p.shop_id = s2.id
    out << "fk change: #{p.shop.name}"
    p.shop = s
    out << "writer: #{p.shop.equal?(s)}"
    p.reload
    out << "reload: #{p.shop.name} #{p.shop.equal?(s)}"
    shop = Shop.find(s.id)
    shop.products.to_a.first.title = "edited"
    Product.create!(shop: s, title: "outside")
    out << "has_many cached: #{shop.products.map(&:title).inspect}"
    shop.reload
    out << "has_many reload: #{shop.products.map(&:title).sort.inspect}"
    Profile.create!(shop: s, bio: "b")
    shop.profile.bio = "edited"
    out << "has_one cached: #{shop.profile.bio}"
    out
  end
end
"##;

const PLAIN_EXPECTED: &[&str] = &[
    "unsaved owner: unsaved true",
    "belongs_to cached: changed true",
    "same key: changed",
    "fk change: two",
    "writer: true",
    "reload: db false",
    "has_many cached: [\"edited\"]",
    "has_many reload: [\"outside\", \"p\"]",
    "has_one cached: edited",
];

/// An app whose only reflection read is `association(:shop).loaded?`,
/// which `lower::assoc_loaded` collapses onto the flat `shop_loaded?`:
/// a key write keeps a loaded target loaded (stale) as Rails' does,
/// and `Product.new(shop: …)` holds the target its writer loaded.
const LOADED_ONLY_PROBE: &str = r##"class CacheProbe
  def self.lines
    out = []
    held = Product.new(shop: Shop.new(name: "unsaved"), title: "f")
    out << "new with target: #{held.association(:shop).loaded?} #{held.shop.name}"
    s = Shop.create!(name: "one")
    s2 = Shop.create!(name: "two")
    p = Product.find(Product.create!(shop: s, title: "p").id)
    before = p.association(:shop).loaded?
    p.shop
    out << "read: #{before} #{p.association(:shop).loaded?}"
    p.shop_id = s2.id
    out << "fk change: #{p.association(:shop).loaded?} #{p.shop.name} #{p.association(:shop).loaded?}"
    p.reload
    out << "reload: #{p.association(:shop).loaded?}"
    out
  end
end
"##;

const LOADED_ONLY_EXPECTED: &[&str] =
    &["new with target: true unsaved", "read: false true", "fk change: true two true", "reload: false"];

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table "shops", force: :cascade do |t|
    t.string "name"
  end
  create_table "products", force: :cascade do |t|
    t.integer "shop_id"
    t.string "title"
  end
  create_table "profiles", force: :cascade do |t|
    t.integer "shop_id"
    t.string "bio"
  end
end
"#;

const STRUCTURE: &str = r#"CREATE TABLE shops (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
CREATE TABLE products (
    id bigserial NOT NULL,
    shop_id bigint,
    title character varying,
    CONSTRAINT products_pkey PRIMARY KEY (id)
);
CREATE TABLE profiles (
    id bigserial NOT NULL,
    shop_id bigint,
    bio character varying,
    CONSTRAINT profiles_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_association_cache\n";

fn models(probe: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/shop.rb", SHOP),
        ("app/models/product.rb", PRODUCT),
        ("app/models/profile.rb", PROFILE),
        ("app/models/cache_probe.rb", probe),
    ]
}

fn native_sqlite(label: &str, files: &[(&str, &str)]) -> Vec<String> {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let dir = std::env::temp_dir().join(format!("rh_association_cache_{label}_native_{}", std::process::id()));
    for (path, text) in files.iter().chain([&("db/schema.rb", SCHEMA)]) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let loads: String = files.iter().map(|(path, _)| format!("load {path:?}\n")).collect();
    let program = format!(
        "require {oracle:?}\nRailsOracle.activate(\"activerecord\", sqlite3: \"2.9.2\")\nrequire \"active_support/all\"\nrequire \"active_record\"\n\
         ActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\n\
         ActiveRecord::Migration.verbose = false\nload \"db/schema.rb\"\n{loads}puts CacheProbe.lines\n"
    );
    std::fs::write(dir.join("native.rb"), program).unwrap();
    let out = Command::new("ruby")
        .arg("native.rb")
        .current_dir(&dir)
        .env("GLIBC_TUNABLES", "glibc.rtld.optional_static_tls=262144")
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native run failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout.lines().map(str::to_string).collect()
}

/// The same files emitted on the SQLite CRuby tree.
fn emitted_sqlite(files: &[(&str, &str)]) -> Vec<String> {
    let app = files
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
        .write("db/schema.rb", SCHEMA)
        .write("config/database.yml", "test:\n  adapter: sqlite3\n  database: \":memory:\"\n");
    let (dir, errors) = app.emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let script = "require File.expand_path('main', Dir.pwd)\nMain.configure_default_adapter!\nputs CacheProbe.lines\n";
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args(["-W0", "-e", script])
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{stdout}\n{}\n{}",
        String::from_utf8_lossy(&out.stderr),
        std::fs::read_to_string(dir.join("app/models/product.rb")).unwrap_or_default()
    );
    stdout.lines().map(str::to_string).collect()
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn association_readers_cache_like_activerecord_on_sqlite() {
    let files = models(PROBE);
    let native = native_sqlite("objects", &files);
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_sqlite(&files), native, "emitted");
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn association_readers_cache_without_association_objects_on_sqlite() {
    let files = models(PLAIN_PROBE);
    let native = native_sqlite("plain", &files);
    assert_eq!(native, PLAIN_EXPECTED, "native");
    assert_eq!(emitted_sqlite(&files), native, "emitted");
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn belongs_to_loaded_answers_a_stale_target_on_sqlite() {
    let files = models(LOADED_ONLY_PROBE);
    let native = native_sqlite("loaded_only", &files);
    assert_eq!(native, LOADED_ONLY_EXPECTED, "native");
    assert_eq!(emitted_sqlite(&files), native, "emitted");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn association_readers_cache_like_activerecord_on_postgresql() {
    let mut files = models(PROBE);
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
        "app/models/product",
        "app/models/profile",
        "app/models/cache_probe",
    ];
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "association-cache",
        &files,
        &emitted,
        "rh_spec_association_cache",
        "CacheProbe",
        &loads,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
