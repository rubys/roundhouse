//! Physical SQL names come from schema/Model.table metadata, not Ruby
//! identifiers. These tests do not widen literal self.table_name admission.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::shared::schema_sql::render_schema_sql;
use roundhouse::emit::shared::seed_sql::{render_schema_only_sql, render_seed_sql};
use roundhouse::ident::{Symbol, TableRef};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{BuildTarget, target_files, write_to_dir};

const CASES: &[(&str, &str, &str)] = &[
    ("Purchase", "purchases", "index_purchases_on_name"),
    ("ReservedPurchase", "order", "select"),
    ("HyphenPurchase", "legacy-entries", "legacy-index"),
    ("SpacePurchase", "legacy entries", "legacy index"),
    ("QuotePurchase", "legacy\"entries\"", "legacy\"index\""),
    ("ApostrophePurchase", "legacy'entries", "legacy'index"),
    ("DotPurchase", "legacy.entries", "legacy.index"),
    ("DoubleSpacePurchase", "legacy  entries", "legacy  index"),
    ("DelimiterPurchase", "edge\"#`\\entries", "edge\"#`\\index"),
    ("InterpolationPurchase", "legacy#{1 + 2}", "index#{7 + 9}"),
    (
        "BackslashMarkerPurchase",
        r"legacy\#{1 + 2}",
        r"index\#{7 + 9}",
    ),
    (
        "InjectionPurchase",
        "\"; DELETE FROM purchases; --",
        "\"; DROP TABLE purchases; --",
    ),
];

fn app() -> roundhouse::App {
    let mut tree = HashMap::new();
    let mut schema = String::from("ActiveRecord::Schema.define do\n");
    let mut seeds = String::new();
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"
            .to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        b"class ApplicationController < ActionController::Base\nend\n".to_vec(),
    );
    for (class, _, _) in CASES {
        let table = roundhouse::naming::pluralize_snake(class);
        schema.push_str(&format!(
            "  create_table {table:?} do |t|\n    t.string \"name\", null: false\n    t.string \"index\"\n    t.index [\"name\"], name: \"index_{table}_on_name\"\n  end\n"
        ));
        seeds.push_str(&format!("{class}.create!(name: \"seed '{class}\")\n"));
        let model = format!(
            "class {class} < ApplicationRecord\n  def self.filtered(name)\n    {class}.where(name: name).order(:id).to_a\n  end\n  def self.ordered_indexes\n    {class}.order(index: :asc).pluck(:index)\n  end\nend\n"
        );
        tree.insert(
            PathBuf::from(format!(
                "app/models/{}.rb",
                roundhouse::naming::snake_case(class)
            )),
            model.into_bytes(),
        );
    }
    schema.push_str("end\n");
    tree.insert(PathBuf::from("db/schema.rb"), schema.into_bytes());
    tree.insert(PathBuf::from("db/seeds.rb"), seeds.into_bytes());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest synthetic ordinary models");
    // Change the legitimate physical metadata before analysis/lowering.
    // No quote-dependent literal overrides are admitted by this fixture.
    for (class, physical, index) in CASES {
        let old = Symbol::from(roundhouse::naming::pluralize_snake(class));
        let mut table = app.schema.tables.shift_remove(&old).expect("schema table");
        table.name = Symbol::from(*physical);
        table.indexes[0].name = Symbol::from(*index);
        app.schema.tables.insert(table.name.clone(), table);
        app.models
            .iter_mut()
            .find(|m| m.name.0.as_str() == *class)
            .unwrap()
            .table = TableRef(Symbol::from(*physical));
    }
    app
}

fn run(command: &mut Command) -> String {
    let output = command.output().expect("execute installed test runtime");
    assert!(
        output.status.success(),
        "{command:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).unwrap()
}

fn interpolation_probe() -> roundhouse::expr::Expr {
    use roundhouse::expr::{Expr, ExprNode, InterpPart};
    Expr::new(
        roundhouse::span::Span::synthetic(),
        ExprNode::StringInterp {
            parts: vec![
                InterpPart::Text {
                    value: r"literal\#{1 + 2} / ".into(),
                },
                InterpPart::Expr {
                    expr: roundhouse::lower::typing::lit_int(7),
                },
            ],
        },
    )
}

#[test]
fn literal_markers_are_data_not_expression_interpolation() {
    use roundhouse::expr::{Expr, ExprNode, Literal};
    for (value, expected) in [
        ("literal#{1 + 2}", r#""literal\#{1 + 2}""#),
        (r"literal\#{1 + 2}", r#""literal\\\#{1 + 2}""#),
    ] {
        let expr = Expr::new(
            roundhouse::span::Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Str {
                    value: value.into(),
                },
            },
        );
        assert_eq!(
            roundhouse::emit::crystal::emit_expr_for_runtime(&expr),
            expected
        );
    }
    assert_eq!(
        roundhouse::emit::crystal::emit_expr_for_runtime(&interpolation_probe()),
        r#""literal\\\#{1 + 2} / #{7_i64}""#
    );
    let elixir = roundhouse::emit::elixir::emit(&app())
        .into_iter()
        .find(|f| f.path == Path::new("lib/roundhouse/schema_sql.ex"))
        .unwrap();
    assert!(elixir.content.contains(r"legacy\#{1 + 2}"));
    assert!(elixir.content.contains(r"legacy\\\#{1 + 2}"));
}

#[test]
fn one_identifier_preserves_spelling_and_escapes_quotes() {
    for (raw, expected) in [
        ("purchases", "purchases"),
        ("_Purchase2", "_Purchase2"),
        ("OrDeR", "\"OrDeR\""),
        ("legacy-entries", "\"legacy-entries\""),
        ("legacy entries", "\"legacy entries\""),
        ("legacy\"entries\"", "\"legacy\"\"entries\"\"\""),
        ("legacy.entries", "\"legacy.entries\""),
        ("2purchases", "\"2purchases\""),
        ("käibed", "\"käibed\""),
    ] {
        assert_eq!(roundhouse::naming::sql_ident(raw), expected, "{raw}");
    }
}

#[test]
fn ddl_and_seeds_execute_in_the_exact_tables() {
    let app = app();
    let payload = serde_json::json!({
        "ddl": render_schema_sql(&app.schema),
        "schema_only": render_schema_only_sql(&app).unwrap(),
        "seed": render_seed_sql(&app).expect("literal model seed"),
        "cases": CASES,
    });
    let stdout = run(Command::new("python3").args([
        "-c",
        r#"
import json, sqlite3, sys
p = json.loads(sys.argv[1])
for key in ('ddl', 'schema_only', 'seed'):
    db = sqlite3.connect(':memory:')
    db.executescript(p[key])
    db.executescript(p['ddl']) # idempotent without silently changing names
    assert set(db.execute("SELECT name FROM sqlite_schema WHERE type='table'")) == {
        (t,) for _, t, _ in p['cases']
    } | {('sqlite_sequence',)}
    assert set(db.execute("SELECT name, tbl_name FROM sqlite_schema WHERE type='index'")) == {
        (idx, t) for _, t, idx in p['cases']
    }
    for cls, table, _ in p['cases']:
        # Independent query spelling, not Roundhouse's renderer.
        rows = db.execute('SELECT id, name FROM "' + table.replace('"', '""') + '"').fetchall()
        assert rows == ([(1, "seed '" + cls)] if key == 'seed' else [])
print('exact DDL/index names and isolated seed values passed')
"#,
        &payload.to_string(),
    ]));
    assert!(stdout.contains("isolated seed values passed"));
}

#[test]
fn virtual_table_name_is_quoted_without_rewriting_module_arguments() {
    let mut app = app();
    app.schema.tables.clear();
    let name = Symbol::from("virtual \"order\"");
    app.schema.tables.insert(
        name.clone(),
        roundhouse::schema::Table {
            name,
            columns: vec![],
            indexes: vec![],
            foreign_keys: vec![],
            virtual_module: Some(roundhouse::schema::VirtualModule {
                module: "fts5".into(),
                args: vec!["body".into(), "tokenize=porter".into()],
            }),
        },
    );
    let ddl = render_schema_sql(&app.schema);
    let stdout = run(Command::new("python3").args(["-c", r#"
import sqlite3, sys
db = sqlite3.connect(':memory:')
db.executescript(sys.argv[1])
db.execute('INSERT INTO "virtual ""order""" (body) VALUES (?)', ('running',))
assert db.execute('SELECT body FROM "virtual ""order""" WHERE body MATCH ?', ('run',)).fetchall() == [('running',)]
assert db.execute("SELECT name FROM sqlite_schema WHERE sql LIKE 'CREATE VIRTUAL TABLE%'").fetchall() == [('virtual "order"',)]
print('quoted virtual table and porter tokenizer executed')
"#, &ddl]));
    assert!(stdout.contains("porter tokenizer executed"));
}

#[test]
fn emitted_models_execute_crud_and_qualified_projections() {
    let mut app = app();
    let diagnostics = roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<_> = diagnostics
        .into_iter()
        .chain(roundhouse::analyze::diagnose(&app))
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "synthetic app errors: {errors:?}");
    let root =
        std::env::temp_dir().join(format!("roundhouse-sql-identifiers-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let files = target_files(&app, Path::new("."), BuildTarget::Ruby).expect("Ruby project");
    write_to_dir(&files, &root).unwrap();
    let cases = serde_json::to_string(CASES).unwrap();
    let stdout = run(Command::new("ruby").current_dir(&root).env("BLOG_DB", ":memory:").args(["-rjson", "-e", r#"
require_relative 'runtime/db'
require_relative 'runtime/sqlite_adapter'
require_relative 'runtime/active_support_ext'
require_relative 'runtime/active_record'
require_relative 'runtime/active_record_bang'
require_relative 'config/schema'
Dir['app/models/*.rb'].sort.each { |p| require_relative p }
SqliteAdapter.configure(':memory:')
ActiveRecord.adapter = SqliteAdapter
Schema.statements.each { |sql| SqliteAdapter.execute_ddl(sql) }
cases = JSON.parse(ARGV[0])
def check(actual, expected)
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless actual == expected
end
cases.each_with_index do |(class_name, table, _), i|
  model = Object.const_get(class_name)
  check(model.table_name, table) # raw metadata remains raw
  record = model.create!(name: "O'Brien \"quoted\" -- #{i}", index: 'zulu')
  check(record.id, 1)
end
cases.each_with_index do |(class_name, table, _), i|
  model = Object.const_get(class_name)
  value = "O'Brien \"quoted\" -- #{i}"
  check(model.find(1).name, value)
  check(model.create!(name: value, index: 'alpha').id, 2)
  check(model.create!(name: 'other', index: 'middle').id, 3)
  check(model.ordered_indexes, ['alpha', 'middle', 'zulu'])
  check(model.filtered(value).map(&:id), [1, 2])
  check(model.filtered(value).map(&:name), [value, value])
  check(model.filtered('not there'), [])
  check(model.all.map(&:name), [value, value, 'other'])
  check(model.count, 3)
  check(model._adapter_exists_by_id?(1), true)
  check(model._adapter_exists_by_id?(99), false)
  # Independently construct a qualified source, exercising separately
  # quoted table, column and output alias in the generated _columns_sql.
  source = '"' + table.gsub('"', '""') + '"'
  check(model._table_sql, table == 'purchases' ? 'purchases' : source)
  check(model._hydrate_all("SELECT #{model._columns_sql} FROM #{source} ORDER BY id DESC").map(&:name), ['other', value, value])
  begin
    ActiveRecord::Relation.new(model).find(99)
    raise 'missing record did not raise'
  rescue ActiveRecord::RecordNotFound => e
    check(e.message, "Couldn't find record in #{table} with id=99")
  end
  [:first!, :find_by!].each do |method|
    relation = ActiveRecord::Relation.new(model).where(name: 'absent')
    begin
      method == :first! ? relation.first! : relation.find_by!(name: 'also absent')
      raise 'empty relation did not raise'
    rescue ActiveRecord::RecordNotFound => e
      check(e.message, "Couldn't find record in #{table}")
    end
  end
  model.find(2).destroy!
  model.find(3).destroy!
  record = model.find(1)
  record.update!(name: "changed '#{i}")
  record.name = 'not saved'
  record.reload
  check(record.name, "changed '#{i}")
  check(SqliteAdapter.select_rows("SELECT name FROM #{source}").map { |r| r['name'] }, ["changed '#{i}"])
  record.destroy!
  check(model.count, 0)
  record = model.create!(name: 'second')
  check(record.id, 4)
  model.delete_all
  check(model.count, 0)
  check(model.create!(name: 'third').id, 5) # delete_all preserves sequence
  model._adapter_truncate
  check(model.create!(name: 'reset').id, 1)
  check(SqliteAdapter.select_rows('SELECT name, seq FROM sqlite_sequence').map { |r| [r['name'], r['seq']] }.sort,
        cases.map { |(_, t, _)| [t, 1] }.sort)
  # Controls in every other table remain untouched, including sequence.
  cases[i+1..].each do |(other, _, _)|
    check(Object.const_get(other).find(1).name, "O'Brien \"quoted\" -- #{cases.index { |c| c[0] == other }}")
  end
end
puts 'emitted SQLite CRUD, filtered reads, reload, aliases and isolation passed'
"#, &cases]));
    assert!(stdout.contains("aliases and isolation passed"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ddl_survives_python_and_rust_source_literals() {
    let app = app();
    let expected = render_schema_sql(&app.schema);
    let root = std::env::temp_dir().join(format!("roundhouse-sql-literals-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let python = roundhouse::emit::python::emit(&app)
        .into_iter()
        .find(|f| f.path == Path::new("app/schema_sql.py"))
        .expect("Python DDL module");
    let stdout = run(Command::new("python3").args([
        "-c",
        r#"
import sqlite3, sys
namespace = {}
exec(sys.argv[1], namespace)
ddl = namespace['CREATE_TABLES']
db = sqlite3.connect(':memory:')
db.executescript(ddl)
sys.stdout.write(ddl)
"#,
        &python.content,
    ]));
    assert_eq!(stdout, expected);

    let rust = roundhouse::emit::rust::emit(&app)
        .into_iter()
        .find(|f| f.path == Path::new("src/schema_sql.rs"))
        .expect("Rust DDL module");
    let source = root.join("ddl.rs");
    let binary = root.join("ddl");
    std::fs::write(
        &source,
        format!(
            "{}\nfn main() {{ print!(\"{{}}\", CREATE_TABLES); }}\n",
            rust.content
        ),
    )
    .unwrap();
    run(Command::new("rustc").arg(&source).arg("-o").arg(&binary));
    assert_eq!(run(&mut Command::new(&binary)), expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires Go toolchain; runs only the generated DDL constant, not a full app"]
fn ddl_survives_go_source_literal() {
    let app = app();
    let expected = render_schema_sql(&app.schema);
    let root =
        std::env::temp_dir().join(format!("roundhouse-sql-go-literal-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let go = roundhouse::emit::go::emit(&app)
        .into_iter()
        .find(|f| f.path.ends_with("schema_sql.go"))
        .expect("Go DDL module");
    let source = root.join("ddl.go");
    std::fs::write(
        &source,
        format!(
            "{}\nfunc main() {{ fmt.Print(CreateTables) }}\n",
            go.content
                .replace("package v2", "package main\nimport \"fmt\"")
        ),
    )
    .unwrap();
    assert_eq!(run(Command::new("go").arg("run").arg(&source)), expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires Crystal; evaluates generated Schema and literal/interpolation expressions only"]
fn ddl_survives_crystal_source_literals() {
    let app = app();
    let expected = render_schema_sql(&app.schema);
    let root = std::env::temp_dir().join(format!("roundhouse-sql-crystal-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let crystal = roundhouse::emit::crystal::emit(&app)
        .into_iter()
        .find(|f| f.path == Path::new("src/schema.cr"))
        .expect("Crystal Schema module");
    let source = root.join("ddl.cr");
    std::fs::write(
        &source,
        format!(
            "{}\nprint Schema.statements.join(\";\\n\") + \";\\n\"\n",
            crystal.content
        ),
    )
    .unwrap();
    assert_eq!(
        run(Command::new("crystal").arg("run").arg(&source)),
        expected
    );
    let code = roundhouse::emit::crystal::emit_expr_for_runtime(&interpolation_probe());
    assert_eq!(
        run(Command::new("crystal").args(["eval", &format!("print {code}")])),
        r"literal\#{1 + 2} / 7"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires Elixir; evaluates generated DDL and interpolation only, not a whole app"]
fn ddl_survives_elixir_source_literal() {
    let app = app();
    let expected = render_schema_sql(&app.schema);
    let elixir = roundhouse::emit::elixir::emit(&app)
        .into_iter()
        .find(|f| f.path == Path::new("lib/roundhouse/schema_sql.ex"))
        .expect("Elixir DDL module");
    assert_eq!(
        run(Command::new("elixir").env("ERL_FLAGS", "+S 2").args([
            "-e",
            &format!(
                "{}\nIO.write(Roundhouse.SchemaSQL.create_tables())",
                elixir.content
            )
        ])),
        expected
    );
    let code = roundhouse::emit::elixir::format_constant("probe", &interpolation_probe());
    assert_eq!(run(Command::new("elixir").env("ERL_FLAGS", "+S 2").args(["-e",
        &format!("defmodule LiteralProbe do\n{code}\ndef probe, do: @probe\nend\nIO.write(LiteralProbe.probe())")])), r"literal\#{1 + 2} / 7");
}
