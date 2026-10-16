use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::{ingest_model, ingest_schema};
use roundhouse::App;

fn emit(body: &str) -> String {
    let schema = ingest_schema(
        br#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "events", force: :cascade do |t|
    t.string "starts_on"
  end
end
"#,
        "db/schema.rb",
    )
    .expect("ingest schema");
    let src = format!("class Event < ApplicationRecord\n  def probe\n    {body}\n  end\nend\n");
    let model = ingest_model(src.as_bytes(), "app/models/event.rb", &schema, &Default::default())
        .expect("ingest model")
        .expect("model recognized");
    let mut app = App::new();
    app.models.push(model);
    app.schema = schema;
    Analyzer::new(&app).analyze(&mut app);
    emit_lowered_models(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn time_parse_grounds_to_the_runtime_parser() {
    let out = emit("Time.parse(starts_on)");
    assert!(out.contains("ActiveSupport.parse_time(self.starts_on)"), "{out}");
    assert!(!out.contains("Time.parse"), "{out}");
}

#[test]
fn time_zone_parse_grounds_to_the_zone_parser() {
    let out = emit("Time.zone.parse(starts_on)");
    assert!(out.contains("ActiveSupport.zone_parse(self.starts_on)"), "{out}");
    assert!(!out.contains("Time.zone"), "{out}");
}

#[test]
fn the_two_argument_form_is_left_alone() {
    let out = emit("Time.zone.parse(starts_on, Time.now)");
    assert!(out.contains("Time.zone.parse(self.starts_on, Time.now)"), "{out}");
    assert!(!out.contains("zone_parse"), "{out}");
}
