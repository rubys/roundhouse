use roundhouse::App;
use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::dialect::ModelBodyItem;
use roundhouse::emit::diagnostics::scope;
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{BuildTarget, target_files};
use roundhouse::ty::Ty;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "calendar_entries" do |t|
    t.date "due_on"
    t.datetime "observed_at"
    t.time "opens_at"
  end
end
"#;

fn app_with(schema: &str, model: &str) -> App {
    let tree = [
        ("db/schema.rb", schema),
        ("app/models/calendar_entry.rb", model),
    ]
    .into_iter()
    .map(|(p, s)| (p.into(), s.as_bytes().to_vec()))
    .collect();
    ingest_app_from_tree(tree).expect("independent synthetic app")
}

fn errors(app: &mut App) -> Vec<String> {
    let lower = roundhouse::session::analyze_and_lower(app);
    roundhouse::analyze::diagnose(app)
        .into_iter()
        .chain(lower)
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message)
        .collect()
}

#[test]
fn columns_constructors_and_callers_preserve_the_date_domain() {
    let mut app = app_with(SCHEMA, include_str!("date_columns_model.rb"));
    let row = &app.models[0].attributes;
    assert_eq!(
        row.fields[&Symbol::from("due_on")],
        Ty::Union {
            variants: vec![Ty::Date, Ty::Nil]
        }
    );
    for name in ["observed_at", "opens_at"] {
        assert_eq!(
            row.fields[&Symbol::from(name)],
            Ty::Union {
                variants: vec![Ty::Time, Ty::Nil]
            }
        );
    }
    assert_eq!(errors(&mut app), Vec::<String>::new());
    let method = |name: &str| {
        app.models[0]
            .body
            .iter()
            .find_map(|item| match item {
                ModelBodyItem::Method { method, .. } if method.name.as_str() == name => {
                    Some(method)
                }
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(method("parsed_date").body.ty, Some(Ty::Date));
    let roundhouse::expr::ExprNode::Seq { exprs } = &*method("reset_date").body.node else {
        panic!("expected assignment followed by an analyzed call")
    };
    assert_eq!(
        exprs.last().unwrap().ty,
        Some(Ty::Union {
            variants: vec![Ty::Date, Ty::Nil]
        })
    );
    assert_eq!(roundhouse::ide::render_ty(&Ty::Date), "Date");
    let rbs = "class CalendarEntry\n  def identity: (Date date) -> Date\nend\n";
    let signatures = roundhouse::rbs::parse_app_signatures(rbs).unwrap();
    let sig = &signatures[&ClassId(Symbol::from("CalendarEntry"))][&Symbol::from("identity")];
    let Ty::Fn { params, ret, .. } = sig else {
        panic!("expected function: {sig:?}")
    };
    assert_eq!(params[0].ty, Ty::Date);
    assert_eq!(**ret, Ty::Date);
}

#[test]
fn typo_and_cross_domain_operations_remain_errors() {
    for body in [
        "due_on&.strftiem(\"%Y\")",
        "due_on&.>>(\"1\")",
        "observed_at&.>>(240)",
        "opens_at&.>>(1)",
    ] {
        let model = format!(
            "class CalendarEntry < ApplicationRecord\n  def probe\n    {body}\n  end\nend\n"
        );
        let mut app = app_with(SCHEMA, &model);
        assert!(!errors(&mut app).is_empty(), "incorrectly accepted {body}");
    }
}

const UNSUPPORTED: &[BuildTarget] = &[
    BuildTarget::Jruby,
    BuildTarget::Roda,
    BuildTarget::Crystal,
    BuildTarget::Elixir,
    BuildTarget::Go,
    BuildTarget::Kotlin,
    BuildTarget::Python,
    BuildTarget::Rust,
    BuildTarget::Swift,
    BuildTarget::CSharp,
    BuildTarget::Typescript,
    BuildTarget::TypescriptWorker,
];

fn assert_rejected(app: &App) {
    for &target in UNSUPPORTED {
        let (result, diagnostics) =
            scope(|| target_files(app, std::path::Path::new("not-a-fixture"), target));
        assert!(result.is_err(), "{target:?} silently emitted Date");
        assert_eq!(diagnostics.len(), 1, "{target:?}: {diagnostics:?}");
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert!(
            matches!(&diagnostics[0].kind, DiagnosticKind::Unsupported { construct, target: Some(t), .. }
            if construct.as_str() == "Date" && t.as_str() == target.as_str()),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn date_target_boundary_rejects_before_reading_or_emitting_files() {
    let mut app = app_with(SCHEMA, include_str!("date_columns_model.rb"));
    assert!(errors(&mut app).is_empty()); // target-independent Date behavior is modeled
    assert_rejected(&app);
    for target in [BuildTarget::Ruby] {
        let (result, diagnostics) =
            scope(|| target_files(&app, roundhouse::fixtures::real_blog(), target));
        assert!(result.is_ok(), "{target:?}: {result:?}");
        assert!(
            !diagnostics.iter().any(
                |d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. }
            if construct.as_str() == "Date")
            ),
            "{target:?}: {diagnostics:?}"
        );
        let files = result.unwrap();
        let model = files
            .iter()
            .find(|(p, _)| p == "app/models/calendar_entry.rb")
            .unwrap();
        assert!(model.1.contains("ActiveSupport.parse_db_date(@due_on_raw)"));
        assert!(model.1.contains("ActiveSupport.format_db_date"));
        assert!(!model.1.contains("present_db(@__t_due_on"));
        assert!(model.1.contains("schema_date_columns"));
    }
}

#[test]
fn spinel_emits_date_runtime_and_keeps_date_as_a_date() {
    let mut app = app_with(SCHEMA, include_str!("date_columns_model.rb"));
    assert!(errors(&mut app).is_empty());
    let (result, diagnostics) =
        scope(|| target_files(&app, std::path::Path::new("not-a-fixture"), BuildTarget::Spinel));
    assert!(result.is_ok(), "{diagnostics:?}");
    let files = result.unwrap();
    let runtime = files.iter().find(|(path, _)| path == "runtime/date.rb").unwrap();
    assert!(runtime.1.contains("class Date"));
    assert!(
        files.iter().any(|(path, _)| path == "runtime/date.rbs"),
        "signature files: {:?}",
        files.iter().map(|(path, _)| path).filter(|path| path.ends_with(".rbs")).collect::<Vec<_>>()
    );
    let boot = files.iter().find(|(path, _)| path == "boot.rb").unwrap();
    assert!(
        boot.1.contains("require_relative \"runtime/date\""),
        "date apps must load the program-defined Date"
    );
    assert!(
        boot.1.contains("require_relative \"runtime/active_support_date_parsing\""),
        "date apps must load Date parse/format intrinsics"
    );
    assert!(
        boot.1.contains("require_relative \"runtime/active_record_date_serialization\""),
        "date apps must load date-aware JSON serialization"
    );
    assert!(
        files.iter().any(|(path, _)| path == "runtime/active_support_date_parsing.rb"),
        "date parse/format must ship with the Date package"
    );
    assert!(
        boot.1.contains("require_relative \"runtime/active_record_serialization\""),
        "default as_json entrypoint is always-on"
    );
    let model = files
        .iter()
        .find(|(path, _)| path == "app/models/calendar_entry.rb")
        .unwrap();
    assert!(model.1.contains("ActiveSupport.parse_db_date(@due_on_raw)"));
    assert!(model.1.contains("ActiveSupport.format_db_date"));
    assert!(!diagnostics.iter().any(|d| matches!(
        &d.kind,
        DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Date"
    )));
}

#[test]
fn spinel_omits_date_runtime_when_the_app_has_no_dates() {
    // Campfire-shaped: no t.date columns and no Date constructors.
    // Loading Date#strftime into every Spinel tree breaks poly
    // Time|Date receivers (matz/spinel#7334) — omit until needed.
    let mut app = app_with(
        r#"ActiveRecord::Schema.define do
  create_table "widgets" do |t|
    t.string "name"
    t.datetime "shipped_at"
  end
end
"#,
        "class Widget < ApplicationRecord\nend\n",
    );
    assert!(errors(&mut app).is_empty());
    let (result, diagnostics) =
        scope(|| target_files(&app, std::path::Path::new("not-a-fixture"), BuildTarget::Spinel));
    assert!(result.is_ok(), "{diagnostics:?}");
    assert!(
        !diagnostics.iter().any(|d| matches!(
            &d.kind,
            DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Date"
        )),
        "{diagnostics:?}"
    );
    let files = result.unwrap();
    assert!(
        !files.iter().any(|(path, _)| path == "runtime/date.rb"),
        "date.rb must not ship when unused"
    );
    assert!(
        !files.iter().any(|(path, _)| path == "runtime/active_support_date_parsing.rb"),
        "date parse/format must not ship when unused"
    );
    assert!(
        !files.iter().any(|(path, _)| path == "runtime/active_record_date_serialization.rb"),
        "date serialization reopen must not ship when unused"
    );
    assert!(
        files.iter().any(|(path, _)| path == "runtime/active_record_serialization.rb"),
        "default as_json entrypoint still ships without dates"
    );
    let boot = files.iter().find(|(path, _)| path == "boot.rb").unwrap();
    assert!(
        !boot.1.contains("runtime/date\""),
        "boot must not require Date when unused:\n{}",
        boot.1
    );
    assert!(
        !boot.1.contains("active_support_date_parsing"),
        "boot must not require date parse/format when unused"
    );
    assert!(
        !boot.1.contains("active_record_date_serialization"),
        "boot must not require date JSON reopen when unused"
    );
    assert!(
        boot.1.contains("require_relative \"runtime/active_record_serialization\""),
        "boot still loads always-on as_json without dates"
    );
}

#[test]
fn non_column_dates_are_also_rejected_on_unimplemented_targets() {
    let mut app = app_with(
        "",
        "class CalendarEntry < ApplicationRecord\n  def probe\n    Date.new(2024, 1, 31) >> 2\n  end\nend\n",
    );
    assert!(errors(&mut app).is_empty());
    assert_rejected(&app);
}

#[test]
fn known_bad_constructor_arguments_remain_errors() {
    for call in [
        "Date.new(\"2024\", 1, 31)",
        "Date.new(2024, 1, 31, Date::GREGORIAN, 0)",
        "Date.parse(17)",
        "Date.iso8601(2024)",
        "Date.strptime(\"2024-01-31\", 17)",
        "Date.today(\"bad\")",
        "Date.current(\"bad\")",
    ] {
        let mut app = app_with(
            "",
            &format!(
                "class CalendarEntry < ApplicationRecord\n  def probe\n    {call}\n  end\nend\n"
            ),
        );
        assert!(!errors(&mut app).is_empty(), "incorrectly accepted {call}");
    }
    for call in [
        "Date.new(2024, 1, 31)",
        "::Date.new(2024, 1, 31)",
        "Date.civil(2024, 2, 29)",
        "Date.parse(\"2024-01-31\", false)",
        "Date.strptime(\"2024-01-31\", \"%Y-%m-%d\")",
        "Date.iso8601(\"2024-01-31\")",
        "Date.today",
        // Rails ActiveSupport: zone-aware, still a Date.
        "Date.current",
        "::Date.current",
    ] {
        let mut app = app_with(
            "",
            &format!(
                "class CalendarEntry < ApplicationRecord\n  def probe\n    {call}\n  end\nend\n"
            ),
        );
        assert_eq!(errors(&mut app), Vec::<String>::new(), "rejected {call}");
    }
}

#[test]
fn date_signatures_normalize_builtins_without_erasing_nominal_namespaces() {
    let rbs = "module Scheduling\n  class Date\n  end\n  class Probe\n    def builtin: (Date x) -> ::Date\n    def nominal: (Scheduling::Date x) -> ::Scheduling::Date\n    def nested: (Array[Date?] xs) -> Hash[String, ::Date]\n  end\nend\n";
    let sigs = roundhouse::rbs::parse_app_signatures(rbs).unwrap();
    let methods = &sigs[&ClassId(Symbol::from("Scheduling::Probe"))];
    let signature = |name: &str| methods[&Symbol::from(name)].clone();
    for (name, expected) in [
        ("builtin", Ty::Date),
        (
            "nominal",
            Ty::Class {
                id: ClassId(Symbol::from("Scheduling::Date")),
                args: vec![],
            },
        ),
    ] {
        let Ty::Fn { params, ret, .. } = signature(name) else {
            panic!("not a function")
        };
        assert_eq!(params[0].ty, expected);
        assert_eq!(*ret, expected);
    }
    let Ty::Fn { params, ret, .. } = signature("nested") else {
        panic!("not a function")
    };
    assert_eq!(
        params[0].ty,
        Ty::Array {
            elem: Box::new(Ty::Union {
                variants: vec![Ty::Date, Ty::Nil]
            })
        }
    );
    assert_eq!(
        *ret,
        Ty::Hash {
            key: Box::new(Ty::Str),
            value: Box::new(Ty::Date)
        }
    );
    let date = Ty::Union {
        variants: vec![Ty::Date, Ty::Nil],
    };
    let text = roundhouse::rbs::print_ty(&date);
    let reparsed = roundhouse::rbs::parse_app_signatures(&format!(
        "class Probe\n  def value: () -> {text}\nend\n"
    ))
    .unwrap();
    let Ty::Fn { ret, .. } = &reparsed[&ClassId(Symbol::from("Probe"))][&Symbol::from("value")]
    else {
        panic!("not a function")
    };
    assert_eq!(**ret, date);

    let app = app_with(
        "",
        "class CalendarEntry < ApplicationRecord\n  sig { params(value: ::Date).returns(Date) }\n  def builtin(value)\n    value\n  end\n  sig { params(value: Scheduling::Date).returns(Scheduling::Date) }\n  def nominal(value)\n    value\n  end\nend\n",
    );
    let sigs = &app.rbs_signatures[&ClassId(Symbol::from("CalendarEntry"))];
    for (name, expected) in [
        ("builtin", Ty::Date),
        (
            "nominal",
            Ty::Class {
                id: ClassId(Symbol::from("Scheduling::Date")),
                args: vec![],
            },
        ),
    ] {
        let Ty::Fn { params, ret, .. } = &sigs[&Symbol::from(name)] else {
            panic!("not a function")
        };
        assert_eq!(params[0].ty, expected);
        assert_eq!(**ret, expected);
    }
}

#[test]
fn date_gate_covers_independent_emitted_roots_before_analysis() {
    for (path, source) in [
        ("db/seeds.rb", "Date.new(2024, 1, 31)\n"),
        ("db/seeds.rb", "::Date.new(2024, 1, 31)\n"),
        (
            "app/controllers/calendar_entries_controller.rb",
            "class CalendarEntriesController < ApplicationController\n  def show\n    Date.new(2024, 1, 31)\n  end\nend\n",
        ),
        (
            "app/controllers/calendar_entries_controller.rb",
            "class CalendarEntriesController < ApplicationController\n  def show(date: Date.new(2024, 1, 31))\n    nil\n  end\nend\n",
        ),
        (
            "app/models/calendar_entry.rb",
            "class CalendarEntry < ApplicationRecord\n  def probe(date = Date.new(2024, 1, 31))\n    nil\n  end\nend\n",
        ),
        (
            "test/models/calendar_entry_test.rb",
            "class CalendarEntryTest < ActiveSupport::TestCase\n  test \"date\" do\n    Date.new(2024, 1, 31)\n  end\nend\n",
        ),
        (
            "test/fixtures/calendar_entries.yml",
            "one:\n  due_on: <%= Date.new(2024, 1, 31) %>\n",
        ),
        (
            "sig/calendar_entry.rbs",
            "class CalendarEntry\n  def unused: (Array[::Date] dates) -> nil\nend\n",
        ),
    ] {
        let tree = [(path, source)]
            .into_iter()
            .map(|(p, s)| (p.into(), s.as_bytes().to_vec()))
            .collect();
        let app = ingest_app_from_tree(tree).unwrap();
        match path {
            "db/seeds.rb" => assert!(app.seeds.is_some()),
            "test/models/calendar_entry_test.rb" => assert_eq!(app.test_modules.len(), 1),
            "test/fixtures/calendar_entries.yml" => assert_eq!(app.fixtures.len(), 1),
            "sig/calendar_entry.rbs" => assert!(!app.rbs_signatures.is_empty()),
            "app/models/calendar_entry.rb" => assert_eq!(app.models.len(), 1),
            _ => assert_eq!(app.controllers.len(), 1),
        }
        assert_rejected(&app);
    }
}

#[test]
fn date_gate_checks_modeled_strict_local_defaults() {
    // Ingest currently only retains literal strict-local defaults.
    // Exercise the boundary's IR contract without claiming Date.new
    // in a source header is supported by that unrelated parser.
    let mut app = ingest_app_from_tree(
        [(
            "app/views/calendar_entries/_entry.html.erb".into(),
            b"<%# locals: (due_on: nil) -%>\n<%= due_on %>\n".to_vec(),
        )]
        .into_iter()
        .collect(),
    )
    .unwrap();
    let source = app_with(
        "",
        "class CalendarEntry < ApplicationRecord\n  def probe\n    Date.new(2024, 1, 31)\n  end\nend\n",
    );
    let ModelBodyItem::Method { method, .. } = &source.models[0].body[0] else {
        panic!("not a method")
    };
    app.views[0].strict_locals.as_mut().unwrap()[0].default = Some(method.body.clone());
    assert_rejected(&app);
}

#[test]
fn date_gate_checks_direct_routes_and_sql_function_bodies() {
    let source = "Rails.application.routes.draw do\n  direct :dated do\n    Date.new(2024, 1, 31).iso8601\n  end\nend\n";
    let app = ingest_app_from_tree(
        [("config/routes.rb".into(), source.as_bytes().to_vec())]
            .into_iter()
            .collect(),
    )
    .unwrap();
    assert_eq!(app.routes.direct_helpers.len(), 1);
    assert_rejected(&app);
    for body in [
        "raw_connection.create_function(\"dated\", 0) do |fn|\n  fn.result = Date.new(2024, 1, 31).iso8601\nend\n",
        "raw_connection.create_aggregate(\"dated\", 1) do\n  step do |fn, value|\n    fn[:date] = Date.new(2024, 1, 31).iso8601\n  end\n  finalize do |fn|\n    fn.result = fn[:date]\n  end\nend\n",
        "raw_connection.create_aggregate(\"dated\", 1) do\n  step do |fn, value|\n    fn[:date] = value\n  end\n  finalize do |fn|\n    fn.result = Date.new(2024, 1, 31).iso8601\n  end\nend\n",
    ] {
        let app = ingest_app_from_tree(
            [(
                "config/initializers/sqlite_functions.rb".into(),
                body.as_bytes().to_vec(),
            )]
            .into_iter()
            .collect(),
        )
        .unwrap();
        assert_eq!(app.sql_functions.len(), 1);
        assert_rejected(&app);
    }
}
