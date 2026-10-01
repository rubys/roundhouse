use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "trades", force: :cascade do |t|
    t.integer "status"
  end
end
"#;

fn emitted() -> String {
    let tree = [
        ("db/schema.rb", SCHEMA.to_string()),
        (
            "app/models/trade.rb",
            "class Trade < ApplicationRecord\n  enum :status, { pending: 0, \"on hold\" => 2 }\n\n  \
             def probe(name)\n    [Trade.statuses[:pending], Trade.statuses[name], Trade.statuses[\"on hold\"], \
             Trade.statuses.key?(:pending)]\n  end\nend\n"
                .to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    emit_lowered_models(&app).into_iter().map(|f| f.content).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_plural_mapping_is_a_class_method_over_every_label() {
    let out = emitted();
    assert!(out.contains("def self.statuses\n    { \"pending\" => 0, \"on hold\" => 2 }\n  end"), "{out}");
}

#[test]
fn a_symbol_key_reads_the_string_label() {
    let out = emitted();
    assert!(out.contains("Trade.statuses[\"pending\"]"), "{out}");
    assert!(out.contains("Trade.statuses[name.to_s]"), "{out}");
    assert!(out.contains("Trade.statuses[\"on hold\"]"), "{out}");
    assert!(out.contains("Trade.statuses.key?(\"pending\")"), "{out}");
}
