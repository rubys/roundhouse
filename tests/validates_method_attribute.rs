//! PostgreSQL: validates method attribute, on the Ruby target against native Rails.
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

const SCHEMA: &str = r#"ActiveRecord::Schema[8.0].define(version: 1) do
  create_table "accounts", force: :cascade do |t|
    t.string "name"
  end
  create_table "documents", force: :cascade do |t|
    t.string "title"
    t.integer "pages"
    t.integer "account_id"
  end
end
"#;

const STRUCTURE: &str = r#"CREATE TABLE accounts (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT accounts_pkey PRIMARY KEY (id)
);

CREATE TABLE documents (
    id bigserial NOT NULL,
    title character varying,
    pages integer,
    account_id bigint,
    CONSTRAINT documents_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const ACCOUNT: &str = "class Account < ApplicationRecord\n  has_many :documents\nend\n";

/// Each rule on a method: presence and absence on predicates (false,
/// true and nil answers), length and format on a reader built from a
/// column, numericality and inclusion on readers that answer nil or a
/// String.
const DOCUMENT: &str = r##"class Document < ApplicationRecord
  belongs_to :account, optional: true
  validates :title, :has_account?, presence: true
  validates :shouting?, absence: true
  validates :label, length: { maximum: 10 }, format: { with: /\A[a-z ]*\z/ }
  validates :pages_text, numericality: { only_integer: true }, allow_nil: true
  validates :kind_name, inclusion: { in: %w[memo note] }

  def has_account?
    account.present?
  end

  def shouting?
    title == "loud" ? true : nil
  end

  def label
    "#{title} doc"
  end

  def pages_text
    return nil if pages.nil?
    pages > 100 ? "many" : pages.to_s
  end

  def kind_name
    title.to_s.start_with?("m") ? "memo" : "other"
  end
end
"##;

/// App code, so the checks run through the compiler. Each line is one
/// record's full messages.
const PROBE: &str = r##"class MethodProbe
  def self.show(out, label, document)
    document.valid?
    out << "#{label}: #{document.errors.full_messages.inspect}"
  end

  def self.lines
    out = []
    account = Account.new(name: "acme")
    show(out, "no account", Document.new(title: "memo", account: nil))
    show(out, "with account", Document.new(title: "memo", account: account))
    show(out, "blank title", Document.new(title: "", account: account))
    show(out, "loud", Document.new(title: "loud", account: account))
    show(out, "long label", Document.new(title: "memorandum", account: account))
    show(out, "bad label", Document.new(title: "Memo", account: account))
    show(out, "many pages", Document.new(title: "memo", pages: 500, account: account))
    show(out, "some pages", Document.new(title: "memo", pages: 12, account: account))
    out
  end
end
"##;

/// Rails' answers, spelled out so a run that drifts from native in the
/// same way on both sides still fails.
const EXPECTED: &[&str] = &[
    r#"no account: ["Has account? can't be blank"]"#,
    r#"with account: []"#,
    r#"blank title: ["Title can't be blank", "Kind name is not included in the list"]"#,
    r#"loud: ["Shouting? must be blank", "Kind name is not included in the list"]"#,
    r#"long label: ["Label is too long (maximum is 10 characters)"]"#,
    r#"bad label: ["Label is invalid", "Kind name is not included in the list"]"#,
    r#"many pages: ["Pages text is not a number"]"#,
    r#"some pages: []"#,
];

const MODELS: &[(&str, &str)] =
    &[("application_record", APPLICATION_RECORD), ("account", ACCOUNT), ("document", DOCUMENT)];

fn app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/method_probe.rb", PROBE);
    for (name, source) in MODELS {
        app = app.write(&format!("app/models/{name}.rb"), source);
    }
    app
}

fn expected() -> String {
    EXPECTED.iter().map(|l| format!("{l}\n")).collect()
}

fn native() -> String {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let dir = std::env::temp_dir().join(format!("rh_validates_method_native_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("schema.rb"), SCHEMA).unwrap();
    std::fs::write(dir.join("method_probe.rb"), PROBE).unwrap();
    let mut requires = String::new();
    for (name, source) in MODELS {
        std::fs::write(dir.join(format!("{name}.rb")), source).unwrap();
        requires.push_str(&format!("require_relative {name:?}\n"));
    }
    let program = format!(
        "require {oracle:?}\nRailsOracle.activate(\"activerecord\", \"activemodel\", \"activesupport\")\n\
         require \"active_record\"\n\
         ActiveRecord::Base.establish_connection(adapter: \"sqlite3\", database: \":memory:\")\n\
         ActiveRecord::Migration.verbose = false\n\
         load File.expand_path(\"schema.rb\", __dir__)\n\
         {requires}require_relative \"method_probe\"\n\
         puts MethodProbe.lines\n"
    );
    std::fs::write(dir.join("native.rb"), program).unwrap();
    let out = Command::new("ruby")
        .args(["-W0", "native.rb"])
        .current_dir(&dir)
        .env("GLIBC_TUNABLES", "glibc.rtld.optional_static_tls=262144")
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native run failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout
}

#[test]
#[ignore = "fork spec: postgresql"]
fn method_attributes_validate_like_rails() {
    let run = app().run_ruby("puts MethodProbe.lines");
    run.assert_passes();
    let document = std::fs::read_to_string(run.emitted.join("app/models/document.rb")).unwrap();
    assert!(!document.contains("@has_account?") && !document.contains("@shouting?"), "{document}");
    assert!(document.contains("ActiveSupport.blank?(self.has_account?)"), "{document}");
    assert!(!document.contains("@label") && !document.contains("@pages_text"), "{document}");
    assert_eq!(run.stdout, expected(), "emitted vs Rails' answers");
    assert_eq!(run.stdout, native(), "emitted vs native activerecord");
}

const DATABASE_YML: &str = "test:\n  adapter: postgresql\n  encoding: unicode\n  database: rh_spec_validates_method\n";

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/account.rb", ACCOUNT),
    ("app/models/document.rb", DOCUMENT),
    ("app/models/method_probe.rb", PROBE),
];

const LOADS: [&str; 4] =
    ["app/models/application_record", "app/models/account", "app/models/document", "app/models/method_probe"];

/// Natively and emitted, on the same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn method_attributes_match_native_rails_on_postgresql() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "validates-method",
        &FILES,
        &emitted,
        "rh_spec_validates_method",
        "MethodProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
