use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "events", force: :cascade do |t|
    t.datetime "starts_at", null: false
    t.datetime "ends_at"
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
    assert_eq!(emit("starts_at.beginning_of_week"), "ActiveSupport.beginning_of_week(starts_at)");
    assert_eq!(emit("starts_at.at_beginning_of_month"), "ActiveSupport.beginning_of_month(starts_at)");
    assert_eq!(emit("starts_at.prev_month"), "ActiveSupport.months_ago(starts_at)");
    assert_eq!(emit("starts_at.next_day(3)"), "ActiveSupport.days_since(starts_at, 3)");
    assert_eq!(emit("starts_at.last_month"), "ActiveSupport.months_ago(starts_at)");
    assert_eq!(emit("starts_at.yesterday?"), "ActiveSupport.yesterday?(starts_at, ActiveSupport.now)");
    assert_eq!(emit("starts_at.monday?"), "ActiveSupport.on_wday?(starts_at, 1)");
}

#[test]
fn a_nullable_reader_grounds_too() {
    assert_eq!(emit("ends_at.end_of_day"), "ActiveSupport.end_of_day(ends_at)");
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
    assert_eq!(emit("starts_at.next_week(:friday)"), "starts_at.next_week(:friday)");
    assert_eq!(emit("starts_at.last_month(2)"), "starts_at.last_month(2)");
}

/// An explicitly seeded Relation retains both inclusive month bounds.
#[test]
fn all_month_is_a_literal_range_for_where_to_render() {
    assert_eq!(
        emit("Event.where(starts_at: starts_at.all_month).count"),
        "ActiveRecord::Relation.new(Event).where(\"(events.starts_at >= ? AND events.starts_at <= ?)\", ActiveSupport.beginning_of_month(starts_at), ActiveSupport.end_of_month(starts_at)).count"
    );
}

/// Range bounds, sibling predicates, and find_by survive explicit seeding.
#[test]
fn a_range_beside_other_keys_splits_into_its_own_where() {
    assert_eq!(
        emit("Event.where(name: name, starts_at: starts_at.all_day).count"),
        "ActiveRecord::Relation.new(Event).where(\"(events.starts_at >= ? AND events.starts_at <= ?)\", ActiveSupport.beginning_of_day(starts_at), ActiveSupport.end_of_day(starts_at)).where(name: name).count"
    );
    assert_eq!(
        emit("Event.find_by(name: name, starts_at: starts_at.all_week)"),
        "ActiveRecord::Relation.new(Event).where(\"(events.starts_at >= ? AND events.starts_at <= ?)\", ActiveSupport.beginning_of_week(starts_at), ActiveSupport.end_of_week(starts_at)).find_by(name: name)"
    );
    assert_eq!(emit("Event.where(name: name).count"), "ActiveRecord::Relation.new(Event).where({ name: name }).count");
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
    assert!(out.starts_with("ActiveSupport.use_zone(name)"), "{out}");
    assert!(out.contains("ActiveSupport.beginning_of_day(starts_at)"), "{out}");
    assert_eq!(
        emit("starts_at.in_time_zone(\"Asia/Tokyo\")"),
        "ActiveSupport.in_time_zone(starts_at, \"Asia/Tokyo\")"
    );
    assert_eq!(emit("starts_at.in_time_zone"), "ActiveSupport.present(starts_at)");
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
