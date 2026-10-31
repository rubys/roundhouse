//! PostgreSQL: association count, on the Ruby target against native Rails.
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
const SHOP: &str = "class Shop < ApplicationRecord\n  has_many :products\nend\n";
const PRODUCT: &str = "class Product < ApplicationRecord\n  belongs_to :shop\nend\n";
const LEDGER: &str = "class Ledger < ApplicationRecord\n  has_many :entries, -> { order(:id) }, inverse_of: :ledger\nend\n";
const ENTRY: &str = "class Entry < ApplicationRecord\n  belongs_to :ledger, inverse_of: :entries\n  validate :within_limit, on: :create\n\n  private\n\n  def within_limit\n    owner = ledger\n    errors.add(:base, \"too many\") unless owner.entries.count < 2\n  end\nend\n";

/// `tally`, `counts` and `lengths` only READ the association, so
/// nothing else in their bodies (a scope, a constructor, a `where`)
/// sends them through the scope-chain rewrite: they are the bodies the
/// bug lived in.
const PROBE: &str = r##"class CountProbe
  def self.lines
    out = []
    s = Shop.create!(name: "one")
    Product.create!(shop: s, title: "a")
    shop = Shop.find(s.id)
    out << "unloaded: #{tally(shop)}"
    Product.create!(shop: s, title: "outside")
    out << "unloaded, outside create: #{tally(shop)}"
    shop.products.create!(title: "through")
    out << "unloaded, create through: #{tally(shop)}"
    out << "unloaded, then length: #{lengths(shop)} #{shop.products.loaded?}"
    loaded = Shop.find(s.id)
    loaded.products.to_a
    out << "loaded: #{tally(loaded)}"
    Product.create!(shop: s, title: "outside 2")
    out << "loaded, outside create: #{tally(loaded)} #{lengths(loaded)}"
    loaded.products.create!(title: "through 2")
    out << "loaded, create through: #{tally(loaded)} #{lengths(loaded)}"
    built = Shop.find(s.id)
    built.products.build(title: "built")
    out << "build: #{built.products.count} #{built.products.size} #{built.products.length}"
    elsewhere = Shop.find(s.id)
    add_unsaved(elsewhere)
    out << "built in another method: #{counts(elsewhere)}"
    out << "validated build: #{build_limited}"
    out << "new owner: #{Shop.new(name: "n").products.count}"
    empty = Shop.find(Shop.create!(name: "empty").id)
    out << "empty: #{counts(empty)}"
    out
  end

  def self.tally(shop)
    "#{counts(shop)} #{shop.products.loaded?}"
  end

  def self.build_limited
    limited = Ledger.create!(name: "limited")
    limited.entries.build(title: "one")
    limited.entries.build(title: "two")
    limited.save!
    "#{limited.entries.count} #{limited.reload.entries.count}"
  rescue ActiveRecord::RecordInvalid => e
    e.message
  end

  def self.add_unsaved(shop)
    shop.products.build(title: "unsaved")
  end

  def self.counts(shop)
    "#{shop.products.count} #{shop.products.size} #{shop.products.empty?} #{shop.products.any?}"
  end

  def self.lengths(shop)
    "#{shop.products.length} #{shop.products.map(&:title).sort.inspect}"
  end
end
"##;

/// What native Rails answers (checked on SQLite and the task
/// PostgreSQL), so two identical wrong answers still fail.
const EXPECTED: &[&str] = &[
    "unloaded: 1 1 false true false",
    "unloaded, outside create: 2 2 false true false",
    "unloaded, create through: 3 3 false true false",
    "unloaded, then length: 3 [\"a\", \"outside\", \"through\"] true",
    "loaded: 3 3 false true true",
    "loaded, outside create: 4 3 false true true 3 [\"a\", \"outside\", \"through\"]",
    "loaded, create through: 5 4 false true true 4 [\"a\", \"outside\", \"through\", \"through 2\"]",
    "build: 5 6 6",
    "built in another method: 5 6 false true",
    "validated build: 2 2",
    "new owner: 0",
    // Not `loaded?` here: Rails' `size` marks an association loaded when
    // its COUNT finds no rows (`count_records`' `loaded! if count == 0`),
    // which the seeded Relation does not tell the owner.
    "empty: 0 0 true false",
];

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table "shops", force: :cascade do |t|
    t.string "name"
  end
  create_table "products", force: :cascade do |t|
    t.integer "shop_id"
    t.string "title"
  end
  create_table "ledgers", force: :cascade do |t|
    t.string "name"
  end
  create_table "entries", force: :cascade do |t|
    t.integer "ledger_id"
    t.string "title"
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
CREATE TABLE ledgers (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT ledgers_pkey PRIMARY KEY (id)
);
CREATE TABLE entries (
    id bigserial NOT NULL,
    ledger_id bigint,
    title character varying,
    CONSTRAINT entries_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str =
    "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_association_count\n";

fn models() -> Vec<(&'static str, &'static str)> {
    vec![
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/shop.rb", SHOP),
        ("app/models/product.rb", PRODUCT),
        ("app/models/ledger.rb", LEDGER),
        ("app/models/entry.rb", ENTRY),
        ("app/models/count_probe.rb", PROBE),
    ]
}

fn native_sqlite(files: &[(&str, &str)]) -> Vec<String> {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let dir = std::env::temp_dir().join(format!("rh_association_count_native_{}", std::process::id()));
    for (path, text) in files.iter().chain([&("db/schema.rb", SCHEMA)]) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let loads: String = files.iter().map(|(path, _)| format!("load {path:?}\n")).collect();
    let program = format!(
        "require {oracle:?}\nRailsOracle.activate(\"activerecord\", sqlite3: \"2.9.2\")\nrequire \"active_support/all\"\nrequire \"active_record\"\n\
         ActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\n\
         ActiveRecord::Migration.verbose = false\nload \"db/schema.rb\"\n{loads}puts CountProbe.lines\n"
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
    let script = "require File.expand_path('main', Dir.pwd)\nMain.configure_default_adapter!\nputs CountProbe.lines\n";
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
        std::fs::read_to_string(dir.join("app/models/count_probe.rb")).unwrap_or_default()
    );
    stdout.lines().map(str::to_string).collect()
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn association_counts_like_activerecord_on_sqlite() {
    let files = models();
    let native = native_sqlite(&files);
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_sqlite(&files), native, "emitted");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn association_counts_like_activerecord_on_postgresql() {
    let mut files = models();
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
    let loads =
        [
        "app/models/application_record",
        "app/models/shop",
        "app/models/product",
        "app/models/ledger",
        "app/models/entry",
        "app/models/count_probe",
    ];
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "association-count",
        &files,
        &emitted,
        "rh_spec_association_count",
        "CountProbe",
        &loads,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}

/// The real-blog probe's authored test, and the loaded-target half of
/// the same sequence, in an emitted test body: there the has_many
/// constructor is lowered by the test lowerer, not the scope chain.
/// The expected numbers are the native answers `EXPECTED` pins for the
/// same reads.
#[test]
#[ignore = "fork spec: postgresql"]
fn create_through_an_association_in_a_test_body_counts_and_joins_the_loaded_target() {
    emit_and_run::real_blog()
        .write(
            "test/models/comment_count_test.rb",
            r#"require "test_helper"

class CommentCountTest < ActiveSupport::TestCase
  test "counts comments of an article" do
    article = articles(:one)
    before = article.comments.count
    article.comments.create!(commenter: "Probe", body: "Another")
    assert_equal before + 1, article.comments.count
  end

  test "a loaded target holds what is created through it" do
    article = articles(:one)
    assert_equal 1, article.comments.to_a.size
    Comment.create!(article_id: article.id, commenter: "Outside", body: "Not in the target")
    comment = article.comments.create!(commenter: "Probe", body: "Another")
    assert_equal "Probe", comment.commenter
    assert_equal 3, article.comments.count
    assert_equal 2, article.comments.size
    assert_equal ["Alice", "Probe"], article.comments.map(&:commenter).sort
  end
end
"#,
        )
        .run_test("test/models/comment_count_test.rb")
        .assert_passes();
}
