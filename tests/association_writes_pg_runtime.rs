//! PostgreSQL: association writes, on the Ruby target against native Rails.
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

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n";

const ORDER: &str = r##"class Order < ApplicationRecord
  validates :name, presence: true
  has_many :lines
  has_many :notes, validate: false
  validate :doubled

  def doubled
    errors.add(:lines, :invalid) if name == "dup"
    errors.add(:lines) if name == "dup"
    errors.add(:lines, "is invalid") if name == "dupstr"
  end
end
"##;

const LINE: &str = r##"class Line < ApplicationRecord
  belongs_to :order, optional: true
  validates :sku, presence: true
  validate :late

  def late
    @checks = (@checks || 0) + 1
    errors.add(:base, "late") if sku == "late" && @checks > 1
  end
end
"##;

const NOTE: &str = r##"class Note < ApplicationRecord
  belongs_to :order
  validates :body, presence: true
end
"##;

const PROBE: &str = r##"class WritesProbe
  def self.counts(base)
    "#{Order.count - base[0]} #{Line.count - base[1]} #{Note.count - base[2]}"
  end

  def self.lines
    out = []
    base = [Order.count, Line.count, Note.count]
    o = Order.create!(name: "o")
    l = Line.create!(sku: "a", order: o)
    l.order = nil
    out << "nil write: #{l.order_id.inspect} #{l.order.inspect}"
    l.save!
    fresh = Line.find(l.id)
    out << "nil saved: #{fresh.order_id.inspect} #{fresh.order.inspect} #{Line.where(order_id: nil).count}"
    m = Line.new(sku: "m", order: nil)
    out << "nil new: #{m.order_id.inspect} #{m.order.inspect} #{m.valid?}"
    m.order = o
    m.order = nil
    m.save!
    out << "nil new saved: #{Line.find(m.id).order_id.inspect} #{Line.where(order_id: nil).count}"
    out << "base: #{counts(base)}"

    a = Order.new(name: "a")
    a.lines.build(sku: "late")
    out << "new owner child fails: #{a.save} #{counts(base)} #{a.persisted?} #{a.new_record?} #{a.errors.full_messages.inspect} #{a.lines.map(&:new_record?).inspect}"
    a.lines.first.sku = "ok"
    out << "new owner resave: #{a.save} #{counts(base)} #{a.persisted?} #{a.lines.map(&:persisted?).inspect}"

    b = Order.new(name: "b")
    b.lines.build(sku: "late")
    begin
      b.save!
      out << "save! no raise"
    rescue ActiveRecord::RecordInvalid => e
      out << "save! raises: #{e.message} #{counts(base)} #{b.new_record?}"
    end

    u = Order.create!(name: "u")
    u.name = "u2"
    u.lines.build(sku: "late")
    out << "update child fails: #{u.save} #{Order.find(u.id).name} #{counts(base)} #{u.persisted?}"

    n = Order.new(name: "n")
    n.notes.build(body: "")
    out << "validate false child fails: #{n.save} #{counts(base)} #{n.persisted?} #{n.notes.map(&:new_record?).inspect}"

    p = Order.create!(name: "p")
    r = (p.lines << Line.new(sku: "x"))
    out << "push persisted: #{counts(base)} #{r.equal?(p.lines)} #{p.lines.map(&:persisted?).inspect} #{p.lines.last.order_id == p.id}"
    p.lines.push(Line.new(sku: "y"))
    out << "push method persisted: #{counts(base)} #{p.lines.size}"
    p.lines << Line.new(sku: "")
    out << "push invalid persisted: #{counts(base)} #{p.lines.size} #{p.lines.map(&:persisted?).inspect}"
    out << "push invalid then save: #{p.save} #{p.errors.full_messages.inspect}"
    fresh = Order.find(p.id)
    fresh.lines << Line.new(sku: "z")
    out << "push unloaded persisted: #{counts(base)} #{fresh.lines.size} #{Order.find(p.id).lines.count}"

    q = Order.new(name: "q")
    q.lines << Line.new(sku: "w")
    out << "push new owner: #{counts(base)} #{q.lines.size}"
    q.save!
    out << "push new owner saved: #{counts(base)} #{q.lines.map(&:persisted?).inspect}"

    t = Order.new(name: "t")
    t.lines.build(sku: "fine")
    t.lines.build(sku: "late")
    out << "second child fails: #{t.save} #{counts(base)} #{t.lines.map(&:new_record?).inspect}"
    t.lines.last.sku = "ok"
    out << "second child resave: #{t.save} #{counts(base)} #{t.lines.map(&:persisted?).inspect}"
    p.lines.push(Line.new(sku: "g"), Line.new(sku: ""))
    out << "push two second invalid: #{counts(base)} #{p.lines.map(&:persisted?).inspect}"
    j = Order.new(name: "j")
    j.lines.build(sku: "late")
    saved = Order.transaction { j.save }
    out << "joined transaction: #{saved.inspect} #{counts(base)} #{j.persisted?}"

    d = Order.new(name: "dup")
    d.lines.build(sku: "")
    out << "dedupe: #{d.valid?} #{d.errors.full_messages.inspect} #{d.errors.count}"
    d = Order.new(name: "dupstr")
    d.lines.build(sku: "")
    out << "dedupe string message: #{d.valid?} #{d.errors.full_messages.inspect} #{d.errors.count}"
    d = Order.new(name: "dup")
    out << "dedupe without records: #{d.valid?} #{d.errors.full_messages.inspect} #{d.errors.count}"
    out
  end
end
"##;

const EXPECTED: &[&str] = &[
    "nil write: nil nil",
    "nil saved: nil nil 1",
    "nil new: nil nil true",
    "nil new saved: nil 2",
    "base: 1 2 0",
    "new owner child fails: false 1 2 0 false true [\"Lines is invalid\"] [true]",
    "new owner resave: true 2 3 0 true [true]",
    "save! raises: Validation failed: Lines is invalid 2 3 0 true",
    "update child fails: false u 3 3 0 true",
    "validate false child fails: true 4 3 0 true [true]",
    "push persisted: 5 4 0 true [true] true",
    "push method persisted: 5 5 0 2",
    "push invalid persisted: 5 5 0 3 [true, true, false]",
    "push invalid then save: false [\"Lines is invalid\"]",
    "push unloaded persisted: 5 6 0 3 3",
    "push new owner: 5 6 0 1",
    "push new owner saved: 6 7 0 [true]",
    "second child fails: false 6 7 0 [true, true]",
    "second child resave: true 7 9 0 [true, true]",
    "push two second invalid: 7 9 0 [true, true, false, false, false]",
    "joined transaction: false 8 9 0 true",
    "dedupe: false [\"Lines is invalid\"] 1",
    "dedupe string message: false [\"Lines is invalid\", \"Lines is invalid\"] 2",
    "dedupe without records: false [\"Lines is invalid\"] 1",
];

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table "orders", force: :cascade do |t|
    t.string "name"
  end
  create_table "lines", force: :cascade do |t|
    t.integer "order_id"
    t.string "sku"
    t.integer "qty"
  end
  create_table "notes", force: :cascade do |t|
    t.integer "order_id"
    t.string "body"
  end
end
"#;

const STRUCTURE: &str = r#"CREATE TABLE orders (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);
CREATE TABLE lines (
    id bigserial NOT NULL,
    order_id bigint,
    sku character varying,
    qty integer,
    CONSTRAINT lines_pkey PRIMARY KEY (id)
);
CREATE TABLE notes (
    id bigserial NOT NULL,
    order_id bigint,
    body character varying,
    CONSTRAINT notes_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str =
    "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_association_writes\n";

const FILES: &[(&str, &str)] = &[
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/order.rb", ORDER),
    ("app/models/line.rb", LINE),
    ("app/models/note.rb", NOTE),
    ("app/models/writes_probe.rb", PROBE),
];

fn native_sqlite() -> Vec<String> {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let dir = std::env::temp_dir().join(format!("rh_association_writes_native_{}", std::process::id()));
    for (path, text) in FILES.iter().chain([&("db/schema.rb", SCHEMA)]) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let loads: String = FILES.iter().map(|(path, _)| format!("load {path:?}\n")).collect();
    let program = format!(
        "require {oracle:?}\nRailsOracle.activate(\"activerecord\", sqlite3: \"2.9.2\")\nrequire \"active_support/all\"\nrequire \"active_record\"\n\
         ActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\n\
         ActiveRecord::Migration.verbose = false\nload \"db/schema.rb\"\n{loads}puts WritesProbe.lines\n"
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
fn emitted_sqlite() -> Vec<String> {
    let app = FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
        .write("db/schema.rb", SCHEMA)
        .write("config/database.yml", "test:\n  adapter: sqlite3\n  database: \":memory:\"\n");
    let (dir, errors) = app.emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let script = "require File.expand_path('main', Dir.pwd)\nMain.configure_default_adapter!\nputs WritesProbe.lines\n";
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
        std::fs::read_to_string(dir.join("app/models/order.rb")).unwrap_or_default()
    );
    stdout.lines().map(str::to_string).collect()
}

#[test]
#[ignore = "fork spec: postgresql native control"]
fn association_writes_like_activerecord_on_sqlite() {
    let native = native_sqlite();
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_sqlite(), native, "emitted");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn association_writes_like_activerecord_on_postgresql() {
    let mut files = FILES.to_vec();
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
    let loads: Vec<&str> = FILES.iter().map(|(path, _)| path.trim_end_matches(".rb")).collect();
    let Some((native, emitted)) = pg_probe::native_and_emitted("association-writes", &files, &emitted, "rh_spec_association_writes", "WritesProbe", &loads) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
