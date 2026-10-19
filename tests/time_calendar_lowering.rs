use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "events", force: :cascade do |t|
    t.datetime "starts_at", null: false
    t.datetime "ends_at"
    t.date "due_on"
    t.string "name"
  end
end
"#;

fn emit(body: &str) -> String {
    let tree = [
        ("db/schema.rb", SCHEMA.to_string()),
        (
            "app/models/event.rb",
            format!("class Event < ApplicationRecord\n  def probe\n    {body}\n  end\nend\n"),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let out = emit_lowered_models(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    let at = out.find("def probe\n").unwrap_or_else(|| panic!("no probe:\n{out}"));
    let body = &out[at + "def probe\n".len()..];
    body[..body.find("\n  end\n").unwrap()].trim().to_string()
}

#[test]
fn calendar_methods_on_a_time_ground_to_module_functions() {
    assert_eq!(emit("starts_at.beginning_of_week"), "ActiveSupport.beginning_of_week(self.starts_at)");
    assert_eq!(emit("starts_at.at_beginning_of_month"), "ActiveSupport.beginning_of_month(self.starts_at)");
    assert_eq!(emit("starts_at.prev_month"), "ActiveSupport.months_ago(self.starts_at)");
    assert_eq!(emit("starts_at.next_day(3)"), "ActiveSupport.days_since(self.starts_at, 3)");
    assert_eq!(emit("starts_at.last_month"), "ActiveSupport.months_ago(self.starts_at)");
    assert_eq!(emit("starts_at.yesterday?"), "ActiveSupport.yesterday?(self.starts_at, ActiveSupport.now)");
    assert_eq!(emit("starts_at.monday?"), "ActiveSupport.on_wday?(self.starts_at, 1)");
}

#[test]
fn time_find_zone_uses_the_nullable_time_zone_lookup() {
    assert_eq!(emit("Time.find_zone(\"Tokyo\")"), "ActiveSupport::TimeZone.lookup(\"Tokyo\")");
    assert_eq!(emit("::Time.find_zone(nil)"), "ActiveSupport::TimeZone.lookup(nil)");
}

#[test]
fn a_nullable_reader_grounds_too() {
    assert_eq!(emit("ends_at.end_of_day"), "ActiveSupport.end_of_day(self.ends_at)");
}

#[test]
fn time_to_date_uses_the_receivers_civil_day() {
    assert_eq!(emit("starts_at.to_date"), "ActiveSupport.date_current(self.starts_at)");
    assert_eq!(
        emit("starts_at.in_time_zone(\"Tokyo\").to_date"),
        "ActiveSupport.date_current(ActiveSupport.in_time_zone(self.starts_at, \"Tokyo\"))"
    );
}

#[test]
fn time_zone_readers_ground_and_chain() {
    assert_eq!(emit("Time.zone.now"), "ActiveSupport.now");
    assert_eq!(emit("Time.zone.yesterday"), "ActiveSupport.yesterday(ActiveSupport.beginning_of_day(ActiveSupport.now))");
    assert_eq!(emit("Time.zone.today.months_ago(2)"), "ActiveSupport.months_ago(ActiveSupport.beginning_of_day(ActiveSupport.now), 2)");
    assert_eq!(emit("Time.zone.now.yesterday.today?"), "ActiveSupport.today?(ActiveSupport.yesterday(ActiveSupport.now), ActiveSupport.now)");
}

#[test]
fn a_form_the_runtime_does_not_take_is_left_alone() {
    assert_eq!(emit("self.starts_at.next_week(:friday)"), "self.starts_at.next_week(:friday)");
    assert_eq!(emit("self.starts_at.last_month(2)"), "self.starts_at.last_month(2)");
}

/// An explicitly seeded Relation retains both inclusive month bounds.
#[test]
fn all_month_is_a_literal_range_for_where_to_render() {
    assert_eq!(
        emit("Event.where(starts_at: starts_at.all_month).count"),
        "ActiveRecord::Relation.new(Event).where(\"(events.starts_at >= ? AND events.starts_at <= ?)\", ActiveSupport.beginning_of_month(self.starts_at), ActiveSupport.end_of_month(self.starts_at)).count"
    );
}

/// Range bounds, sibling predicates, and find_by survive explicit seeding.
#[test]
fn a_range_beside_other_keys_splits_into_its_own_where() {
    assert_eq!(
        emit("Event.where(name: name, starts_at: starts_at.all_day).count"),
        "ActiveRecord::Relation.new(Event).where(\"(events.starts_at >= ? AND events.starts_at <= ?)\", ActiveSupport.beginning_of_day(self.starts_at), ActiveSupport.end_of_day(self.starts_at)).where(name: self.name).count"
    );
    assert_eq!(
        emit("Event.find_by(name: name, starts_at: starts_at.all_week)"),
        "ActiveRecord::Relation.new(Event).where(\"(events.starts_at >= ? AND events.starts_at <= ?)\", ActiveSupport.beginning_of_week(self.starts_at), ActiveSupport.end_of_week(self.starts_at)).find_by(name: self.name)"
    );
    assert_eq!(emit("Event.where(name: name).count"), "ActiveRecord::Relation.new(Event).where({ name: self.name }).count");
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read_dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

#[test]
fn a_controller_where_on_all_month_renders_sql_bounds() {
    let dir = std::env::temp_dir().join("roundhouse-time-calendar-where");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean scratch");
    }
    copy_tree(roundhouse::fixtures::real_blog(), &dir);
    let controller = dir.join("app/controllers/articles_controller.rb");
    let source = std::fs::read_to_string(&controller).expect("read controller").replacen(
        "  def index\n",
        "  def index\n    @n = Article.where(title: \"x\", created_at: Time.zone.now.all_month).count\n",
        1,
    );
    std::fs::write(&controller, source).expect("write controller");

    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, &dir).expect("spinel files");
    let (_, emitted) = files
        .iter()
        .find(|(p, _)| p == "app/controllers/articles_controller.rb")
        .expect("controller emitted");
    assert!(
        emitted.contains(
            "where(\"(articles.created_at >= ? AND articles.created_at <= ?)\", \
             ActiveSupport.beginning_of_month(ActiveSupport.now), ActiveSupport.end_of_month(ActiveSupport.now))\
             .where(title: \"x\")"
        ),
        "{emitted}"
    );
}

fn time_range_residue(body: &str) -> usize {
    let tree = [
        ("db/schema.rb", SCHEMA.to_string()),
        (
            "app/models/event.rb",
            format!("class Event < ApplicationRecord\n  def probe\n    {body}\n  end\nend\n"),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app)
        .iter()
        .filter(|d| matches!(&d.kind,
            roundhouse::diagnostic::DiagnosticKind::LowerResidue { construct, .. } if construct.as_str() == "time_range"))
        .count()
}

#[test]
fn an_all_month_outside_a_condition_is_ledgered() {
    assert_eq!(time_range_residue("month = starts_at.all_month\n    Event.where(starts_at: month).count"), 1);
    assert_eq!(time_range_residue("Event.where(name: name, starts_at: starts_at.all_month).count"), 0);
}

#[test]
fn use_zone_and_in_time_zone_ground_to_the_runtime() {
    let out = emit("Time.use_zone(name) { starts_at.beginning_of_day }");
    assert!(out.starts_with("ActiveSupport.use_zone(self.name)"), "{out}");
    assert!(out.contains("ActiveSupport.beginning_of_day(self.starts_at)"), "{out}");
    assert_eq!(
        emit("starts_at.in_time_zone(\"Asia/Tokyo\")"),
        "ActiveSupport.in_time_zone(self.starts_at, \"Asia/Tokyo\")"
    );
    assert_eq!(emit("starts_at.in_time_zone"), "ActiveSupport.present(self.starts_at)");
}

#[test]
fn calendar_methods_on_a_date_ground_to_date_preserving_functions() {
    assert_eq!(
        emit("Date.current"),
        "ActiveSupport.date_current(ActiveSupport.now)"
    );
    assert_eq!(
        emit("Date.yesterday"),
        "ActiveSupport.date_yesterday(ActiveSupport.date_current(ActiveSupport.now))"
    );
    assert_eq!(
        emit("due_on.beginning_of_month"),
        "ActiveSupport.date_beginning_of_month(self.due_on)"
    );
    assert_eq!(
        emit("due_on.end_of_month"),
        "ActiveSupport.date_end_of_month(self.due_on)"
    );
    assert_eq!(
        emit("due_on.yesterday"),
        "ActiveSupport.date_yesterday(self.due_on)"
    );
    assert_eq!(
        emit("due_on.next_month"),
        "ActiveSupport.date_months_since(self.due_on)"
    );
    assert_eq!(
        emit("due_on.in_time_zone(\"UTC\")"),
        "ActiveSupport.in_time_zone(ActiveSupport.date_at_midnight(self.due_on), \"UTC\")"
    );
    assert_eq!(
        emit("due_on.beginning_of_day"),
        "ActiveSupport.beginning_of_day(ActiveSupport.date_at_midnight(self.due_on))"
    );
    assert_eq!(
        emit("due_on + 2"),
        "ActiveSupport.date_days_since(self.due_on, 2)"
    );
    assert_eq!(
        emit("due_on - 1"),
        "ActiveSupport.date_days_ago(self.due_on, 1)"
    );
    assert_eq!(
        emit("due_on.past?"),
        "ActiveSupport.date_past?(self.due_on, ActiveSupport.date_current(ActiveSupport.now))"
    );
    assert_eq!(
        emit("due_on.today?"),
        "ActiveSupport.date_today?(self.due_on, ActiveSupport.date_current(ActiveSupport.now))"
    );
}

#[test]
fn integer_in_time_zone_grounds_through_epoch() {
    // A bare integer literal in a model method is Ty::Int.
    assert_eq!(
        emit("1.in_time_zone(\"UTC\")"),
        "ActiveSupport.in_time_zone(ActiveSupport.time_at_epoch(1), \"UTC\")"
    );
}

#[test]
fn a_temporal_reader_presents_its_memo_on_every_read() {
    let tree = [
        ("db/schema.rb", SCHEMA.to_string()),
        ("app/models/event.rb", "class Event < ApplicationRecord\nend\n".to_string()),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let out = emit_lowered_models(&app).into_iter().map(|f| f.content).collect::<Vec<_>>().join("\n");
    assert!(
        out.contains("ActiveSupport.present_db(@__t_starts_at ||= ActiveSupport.parse_db_time(@starts_at_raw))"),
        "{out}"
    );
}

fn lowered_probe(body: &str) -> roundhouse::expr::Expr {
    let tree = [
        ("db/schema.rb", SCHEMA.to_string()),
        (
            "app/models/event.rb",
            format!("class Event < ApplicationRecord\n  def probe\n    {body}\n  end\nend\n"),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Event").expect("Event");
    for item in &model.body {
        if let roundhouse::dialect::ModelBodyItem::Method { method, .. } = item {
            if method.name.as_str() == "probe" {
                return method.body.clone();
            }
        }
    }
    panic!("no probe");
}

/// Method names as the lowerer left them, before any emitter prints.
fn ir_shape(e: &roundhouse::expr::Expr) -> String {
    use roundhouse::expr::ExprNode;
    match &*e.node {
        ExprNode::Send { recv, method, args, .. } => {
            let recv = recv.as_ref().map(|r| format!("{}.", ir_shape(r))).unwrap_or_default();
            let args: Vec<String> = args.iter().map(ir_shape).collect();
            if args.is_empty() {
                format!("{recv}{}", method.as_str())
            } else {
                format!("{recv}{}({})", method.as_str(), args.join(", "))
            }
        }
        ExprNode::Const { path } => path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        ExprNode::Lit { value: roundhouse::expr::Literal::Int { value } } => value.to_string(),
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        other => format!("{other:?}"),
    }
}

/// Fails when `Time.now.utc` / zero-arg `iso8601` are renamed only while
/// Crystal prints. The IR already has Crystal's spelling; Crystal's
/// printer does not match on the old names.
#[test]
fn crystal_clock_spelling_is_in_the_ir() {
    let clock = lowered_probe("Time.now.utc.iso8601");
    assert_eq!(ir_shape(&clock), "Time.utc.to_rfc3339", "rename stayed in an emitter");
    assert_eq!(
        roundhouse::emit::crystal::emit_expr_for_runtime(&clock),
        "Time.utc.to_rfc3339"
    );
    // MRI cannot run the IR spelling. Ruby emit prints it back.
    assert_eq!(
        roundhouse::emit::ruby::emit_expr(&clock),
        "Time.now.utc.iso8601"
    );

    let current = lowered_probe("Time.current");
    assert_eq!(ir_shape(&current), "Time.utc");

    let ctor = lowered_probe("Time.utc(2020, 1, 2)");
    assert_eq!(ir_shape(&ctor), "Time.utc(2020, 1, 2)");
    let ctor_crystal = roundhouse::emit::crystal::emit_expr_for_runtime(&ctor);
    assert!(
        ctor_crystal.starts_with("Time.utc(") && !ctor_crystal.contains("now"),
        "{ctor_crystal}"
    );

    let fraction = lowered_probe("starts_at.iso8601(3)");
    assert_eq!(ir_shape(&fraction), "starts_at.iso8601(3)");

    // A namespaced constant is not the stdlib clock.
    let namespaced = lowered_probe("Foo::Time.now.utc");
    assert_eq!(ir_shape(&namespaced), "Foo::Time.now.utc");
    // Ruby prints the chain. An unknown constant is a raise, not `Foo::Time.utc`.
    let ruby = roundhouse::emit::ruby::emit_expr(&namespaced);
    assert!(ruby.contains(".now.utc"), "{ruby}");
    let py = roundhouse::emit::python::emit_expr_for_runtime(&namespaced);
    assert!(!py.contains("datetime.datetime.now"), "{py}");

    let rooted = lowered_probe("::Time.now.utc");
    assert_eq!(ir_shape(&rooted), "::Time.utc");
    assert_eq!(roundhouse::emit::ruby::emit_expr(&rooted), "::Time.now.utc");

    // Zero-arg iso8601 follows the receiver: a time column, not a string,
    // a date, or an untyped call.
    assert_eq!(ir_shape(&lowered_probe("starts_at.iso8601")), "starts_at.to_rfc3339");
    assert_eq!(ir_shape(&lowered_probe("ends_at.iso8601")), "ends_at.to_rfc3339");
    assert_eq!(ir_shape(&lowered_probe("name.iso8601")), "name.iso8601");
    assert_eq!(ir_shape(&lowered_probe("due_on.iso8601")), "due_on.iso8601");
    let unknown = ir_shape(&lowered_probe("mystery.iso8601"));
    assert!(unknown.ends_with("iso8601") && !unknown.contains("to_rfc3339"), "{unknown}");

    // Emitters map `to_rfc3339` only for a time. An application method
    // of that name stays a call. The clock still prints MRI's `iso8601`.
    let custom = lowered_probe("mystery.to_rfc3339");
    assert_eq!(ir_shape(&custom), "mystery.to_rfc3339");
    let custom_ruby = roundhouse::emit::ruby::emit_expr(&custom);
    assert!(custom_ruby.contains("to_rfc3339"), "{custom_ruby}");
    let custom_py = roundhouse::emit::python::emit_expr_for_runtime(&custom);
    assert!(!custom_py.contains("isoformat"), "{custom_py}");
    // `minimum` re-analysis widens `minimum || Time.current` after the
    // rename. The format bit still makes Ruby print `iso8601`.
    let either = emit("(Event.where(name: name).minimum(:starts_at) || Time.current).iso8601");
    assert!(either.contains("iso8601") && !either.contains("to_rfc3339"), "{either}");
    // A source `to_rfc3339` on a mixed union is not that grounded call.
    let mixed = emit("(starts_at || name).to_rfc3339");
    assert!(mixed.contains("to_rfc3339") && !mixed.contains("iso8601"), "{mixed}");

    let column = lowered_probe("starts_at.iso8601");
    let column_py = roundhouse::emit::python::emit_expr_for_runtime(&column);
    assert!(column_py.contains("isoformat"), "{column_py}");
    assert!(
        roundhouse::emit::ruby::emit_expr(&column).contains("iso8601"),
        "time column should print MRI iso8601"
    );
}

/// Test-module constants are outside `for_each_test_body`. The Crystal
/// emitter no longer rewrites the clock, so the constant walk has to.
#[test]
fn a_test_module_constant_gets_the_clock_bridge() {
    let tree = [(
        "test/models/clock_test.rb",
        "class ClockTest < ActiveSupport::TestCase\n  \
         STAMP = Time.now.utc.iso8601\n  \
         OTHER = Foo::Time.now.utc\nend\n",
    )]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let tm = app
        .test_modules
        .iter()
        .find(|t| t.name.0.as_str() == "ClockTest")
        .expect("ClockTest");
    let value = |name: &str| {
        &tm.constants
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .unwrap_or_else(|| panic!("{name} missing: {:?}", tm.constants.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()))
            .1
    };
    assert_eq!(ir_shape(value("STAMP")), "Time.utc.to_rfc3339");
    assert_eq!(ir_shape(value("OTHER")), "Foo::Time.now.utc");
}
