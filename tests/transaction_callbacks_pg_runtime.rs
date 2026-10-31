//! PostgreSQL: transaction callbacks, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE things (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT things_pkey PRIMARY KEY (id)
);
CREATE TABLE gadgets (
    id bigserial NOT NULL,
    name character varying,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    CONSTRAINT gadgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_transaction_callbacks
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const CALLBACK_LOG: &str = r#"class CallbackLog
  def self.entries
    @entries ||= []
  end

  def self.add(entry)
    entries << entry
  end

  def self.take
    taken = entries.join(" ")
    entries.clear
    taken
  end
end
"#;

const PROBE_CALLBACK: &str = r#"class ProbeCallback
  def initialize(tag)
    @tag = tag
  end

  def after_commit(record)
    CallbackLog.add("obj:#{@tag}:#{record.name}")
  end

  def after_rollback(record)
    CallbackLog.add("robj:#{@tag}")
  end
end
"#;

const THING: &str = r#"class Thing < ApplicationRecord
  after_commit :a, on: [:create, :update]
  after_commit :b
  after_save_commit :c
  after_commit :d, only: :create
  after_commit :e, prepend: true
  after_commit ProbeCallback.new(:x), on: :update
  after_create_commit :f
  after_update_commit :f
  after_commit :g, if: [:yes?, :yes?]
  after_commit :h, if: ->(r) { r.name == "z" }
  after_commit :i, unless: :yes?, if: :yes?
  after_commit :j, on: :destroy, unless: -> { name == "keep" }
  before_commit :ba
  before_commit :bb, on: :create
  before_commit :bc, prepend: true
  after_rollback :ra, on: [:create, :update]
  after_rollback :rb
  after_rollback ProbeCallback.new(:y)
  after_save :s

  def a = CallbackLog.add("a")
  def b = CallbackLog.add("b")
  def c = CallbackLog.add("c")
  def d = CallbackLog.add("d")
  def e = CallbackLog.add("e")
  def f = CallbackLog.add("f")
  def g = CallbackLog.add("g")
  def h = CallbackLog.add("h")
  def i = CallbackLog.add("i")
  def j = CallbackLog.add("j")
  def ba = CallbackLog.add("ba")
  def bb = CallbackLog.add("bb")
  def bc = CallbackLog.add("bc")
  def ra = CallbackLog.add("ra")
  def rb = CallbackLog.add("rb")
  def s = CallbackLog.add("s")

  def yes?
    true
  end
end
"#;

/// Callbacks a parent model declares run in its subclass's chain, ahead
/// of the subclass's own.
const TRACKED_RECORD: &str = r#"class TrackedRecord < ApplicationRecord
  self.abstract_class = true

  after_commit :track
  after_rollback :untrack

  def track
    CallbackLog.add("track")
  end

  def untrack
    CallbackLog.add("untrack")
  end
end
"#;

/// The block forms.
const GADGET: &str = r#"class Gadget < TrackedRecord
  after_create_commit { CallbackLog.add("g-create:#{name}") }
  after_commit(on: [:update, :destroy]) do |record|
    %w[n1 n2].each { |n| next if n == "n1"; CallbackLog.add(n) }
    next CallbackLog.add("g-next") if record.name.blank?
    CallbackLog.add("g-ud:#{record.name}")
    CallbackLog.add("g-destroyed:#{record.destroyed?}")
  end
  after_commit -> { CallbackLog.add("g-lambda") }, if: :named?
  after_commit -> do
    CallbackLog.add("g-multi")
    CallbackLog.add("g-multi-end")
  end, on: :update
  before_commit { CallbackLog.add("g-before") }
  after_rollback -> { CallbackLog.add("g-rollback:#{new_record?}") }
  after_destroy_commit ->(record) { CallbackLog.add("g-self:#{self.class.name}:#{record.class.name}") }

  def named?
    name.present?
  end
end
"#;

const PROBE: &str = r##"class CallbackProbe
  def self.run(out, label)
    CallbackLog.take
    yield
    out << "#{label}: #{CallbackLog.take}"
  end

  def self.lines
    out = []
    t = nil
    run(out, "create") { t = Thing.create!(name: "y") }
    run(out, "update") { t.update!(name: "z") }
    run(out, "noop update") { t.save! }
    run(out, "destroy") { t.destroy }
    run(out, "txn create+update") do
      Thing.transaction do
        u = Thing.create!(name: "q")
        u.update!(name: "z")
        CallbackLog.add("in-txn")
      end
    end
    run(out, "rollback") do
      Thing.transaction do
        Thing.create!(name: "r")
        raise ActiveRecord::Rollback
      end
    end
    run(out, "nested savepoint rollback") do
      Thing.transaction do
        Thing.create!(name: "o")
        Thing.transaction(requires_new: true) do
          Thing.create!(name: "i")
          raise ActiveRecord::Rollback
        end
        CallbackLog.add("after-inner")
      end
    end
    run(out, "nested joined") do
      Thing.transaction do
        Thing.transaction { Thing.create!(name: "n") }
        CallbackLog.add("inner-done")
      end
    end
    run(out, "touch without updated_at") { Thing.first.touch }
    v = Thing.new(name: "w")
    run(out, "rolled back create") do
      Thing.transaction do
        v.save!
        raise ActiveRecord::Rollback
      end
    end
    # The id an unsaved record answers: nil on Rails, the runtime's
    # documented unsaved sentinel on the emitted program.
    out << "state: new=#{v.new_record?} persisted=#{v.persisted?} id restored=#{v.id == Thing.new.id}"
    run(out, "save after rollback") { v.save! }
    out << "saved: new=#{v.new_record?} id=#{v.id == Thing.last.id}"
    run(out, "two instances same row") do
      Thing.transaction do
        first = Thing.first
        second = Thing.find(first.id)
        first.update!(name: "1")
        second.update!(name: "2")
      end
    end
    run(out, "exception in txn") do
      begin
        Thing.transaction do
          Thing.create!(name: "e")
          raise "boom"
        end
      rescue => e
        CallbackLog.add(e.message)
      end
    end
    keep = Thing.create!(name: "keep")
    run(out, "destroy unless") { keep.destroy }
    gone = Thing.create!(name: "gone")
    run(out, "rolled back destroy") do
      Thing.transaction do
        gone.destroy
        raise ActiveRecord::Rollback
      end
    end
    out << "destroy state: destroyed=#{gone.destroyed?} persisted=#{gone.persisted?} exists=#{Thing.exists?(gone.id)}"
    run(out, "update_attribute") { gone.update_attribute(:name, "z") }
    g = nil
    run(out, "gadget create") { g = Gadget.create!(name: "p") }
    run(out, "gadget update") { g.update!(name: "") }
    run(out, "gadget touch") { g.touch }
    run(out, "gadget rollback") do
      Gadget.transaction do
        Gadget.create!(name: "r")
        raise ActiveRecord::Rollback
      end
    end
    run(out, "gadget destroy") { g.destroy }
    out << "count: #{Thing.count} #{Gadget.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 25] = [
    "create: s bc ba bb g d c b a e",
    "update: s bc ba h g f obj:x:z d c b a e",
    "noop update: s bc ba h g f obj:x:z d c b a e",
    "destroy: bc ba j h g d b e",
    "txn create+update: s s in-txn bc ba bb h g d c b a e",
    "rollback: s robj:y rb ra",
    "nested savepoint rollback: s s robj:y rb ra after-inner bc ba bb g d c b a e",
    "nested joined: s inner-done bc ba bb g d c b a e",
    "touch without updated_at: bc ba",
    "rolled back create: s robj:y rb ra",
    "state: new=true persisted=false id restored=true",
    "save after rollback: s bc ba bb g d c b a e",
    "saved: new=false id=true",
    "two instances same row: s s bc ba g f obj:x:1 d c b a e",
    "exception in txn: s robj:y rb ra boom",
    "destroy unless: bc ba g d b e",
    "rolled back destroy: robj:y rb",
    "destroy state: destroyed=false persisted=true exists=true",
    "update_attribute: s bc ba h g f obj:x:z d c b a e",
    "gadget create: g-before g-lambda g-create:p track",
    "gadget update: g-before g-multi g-multi-end n2 g-next track",
    "gadget touch: g-before g-multi g-multi-end n2 g-next track",
    "gadget rollback: g-rollback:false untrack",
    "gadget destroy: g-before g-self:Gadget:Gadget n2 g-next track",
    "count: 5 0",
];

/// What native Rails requires, in order.
const LOADS: [&str; 7] = [
    "app/models/callback_log",
    "app/models/probe_callback",
    "app/models/application_record",
    "app/models/thing",
    "app/models/tracked_record",
    "app/models/gadget",
    "app/models/callback_probe",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/callback_log.rb", CALLBACK_LOG),
    ("app/models/probe_callback.rb", PROBE_CALLBACK),
    ("app/models/thing.rb", THING),
    ("app/models/tracked_record.rb", TRACKED_RECORD),
    ("app/models/gadget.rb", GADGET),
    ("app/models/callback_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries: every declaration above is lowered.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

/// The compiler side: the chains, in the order Rails runs them.
#[test]
#[ignore = "fork spec: postgresql"]
fn transaction_callbacks_compile_to_rails_chains() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let thing = std::fs::read_to_string(emitted.join("app/models/thing.rb")).unwrap();
    for expected in [
        "    a if self._transaction_include_create? || self._transaction_include_update?\n",
        "    Thing._thing_transaction_callback_object_5.after_commit self if self._transaction_include_update?\n",
        "  def self._thing_transaction_callback_object_5\n    @@_thing_transaction_callback_object_5 ||= ProbeCallback.new(:x)\n",
        "    g if self.yes? && self.yes?\n",
        "    h if self.name == \"z\"\n",
        "    i if self.yes? && !(self.yes?)\n",
        "    j if self._transaction_include_destroy? && !(self.name == \"keep\")\n",
        "  def _run_before_commit_callbacks\n    bc\n    ba\n    bb if self._transaction_include_create?\n  end\n",
        "  def _run_rollback_callbacks\n    Thing._thing_transaction_callback_object_17.after_rollback self\n    rb\n",
    ] {
        assert!(thing.contains(expected), "missing {expected:?} in:\n{thing}");
    }
    // `after_create_commit :f` is removed by `after_update_commit :f`,
    // `only:` is ignored, and the after chain runs last to first.
    assert_eq!(
        chain(&thing, "_run_commit_callbacks"),
        ["j", "i", "h", "g", "f", "Thing._thing_transaction_callback_object_5.after_commit", "d", "c", "b", "a", "e"],
    );
    let gadget = std::fs::read_to_string(emitted.join("app/models/gadget.rb")).unwrap();
    for expected in ["def _gadget_transaction_callback_1\n", "track\n", "CallbackLog.add(\"g-ud:#{self.name}\")"] {
        assert!(gadget.contains(expected), "missing {expected:?} in:\n{gadget}");
    }
}

/// `config.active_record.run_after_transaction_callbacks_in_order_defined
/// = true` (Rails 7.1 defaults) flips `prepend:` for the after chains, so
/// they run in the order they are declared.
#[test]
#[ignore = "fork spec: postgresql"]
fn in_order_defined_runs_after_callbacks_in_declared_order() {
    let application = "require_relative \"boot\"\nrequire \"rails/all\"\nmodule Probe\n  class Application < Rails::Application\n    config.load_defaults 7.1\n  end\nend\n";
    let (emitted, errors) = overlay().write("config/application.rb", application).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let thing = std::fs::read_to_string(emitted.join("app/models/thing.rb")).unwrap();
    // e (`prepend: true`) is now the one appended, and runs first.
    assert_eq!(
        chain(&thing, "_run_commit_callbacks"),
        ["e", "a", "b", "c", "d", "Thing._thing_transaction_callback_object_5.after_commit", "f", "g", "h", "i", "j"],
    );
    // Only the after chains flip.
    assert_eq!(chain(&thing, "_run_before_commit_callbacks"), ["bc", "ba", "bb"]);
    // 7.1 defaults: before_commit on every instance, commit callbacks on
    // the last saved instance; read by the runtime.
    let app_rb = std::fs::read_to_string(emitted.join("config/application.rb")).unwrap_or_default();
    let all = walk(&emitted);
    assert!(
        all.contains("def active_record_before_committed_on_all_records\n")
            && all.contains("def active_record_run_commit_callbacks_on_first_saved_instances_in_transaction\n"),
        "config readers not synthesized:\n{app_rb}"
    );
}

/// The first word of each statement of the emitted method `name`.
fn chain(source: &str, name: &str) -> Vec<String> {
    let start = source.find(&format!("  def {name}\n")).unwrap_or_else(|| panic!("no {name} in:\n{source}"));
    let body = &source[start..];
    let body = &body[body.find('\n').unwrap() + 1..body.find("\n  end\n").unwrap()];
    body.lines().map(|l| l.trim().split(' ').next().unwrap().to_string()).collect()
}

fn walk(dir: &std::path::Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&walk(&path));
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
    }
    out
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn transaction_callbacks_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "transaction-callbacks",
        &FILES,
        &emitted,
        "rh_spec_transaction_callbacks",
        "CallbackProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
