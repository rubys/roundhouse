//! `attribute :name, :type` (the Rails Attributes API) — typed virtual
//! attributes synthesized in the shared model lowering
//! (`markers::push_attribute_api_methods`): a typed ivar reader plus a
//! writer that applies Rails' Type::Boolean cast for `:boolean` (the
//! form roundtrip assigns "0"/"1" strings; an uncast write leaves "0"
//! truthy). Surfaced by lobsters' `attribute :mod_note, :boolean` on
//! Message: no reader synthesized → f.check_box's typed checked state
//! had nothing to read under spinel AOT.

use roundhouse::dialect::MethodReceiver;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_model_to_library_class;

fn lower_message() -> roundhouse::dialect::LibraryClass {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :messages do |t|\n    t.text :body\n  end\nend\n",
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  attribute :mod_note, :boolean

  def mod_note_label
    "x"
  end
end
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let message = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == "Message")
        .expect("Message model");
    lower_model_to_library_class(message, &app.schema)
}

#[test]
fn boolean_attribute_synthesizes_typed_reader_and_cast_writer() {
    let lc = lower_message();
    let find = |name: &str| {
        lc.methods
            .iter()
            .find(|m| m.name.as_str() == name && m.receiver == MethodReceiver::Instance)
    };

    let reader = find("mod_note").expect("reader synthesized");
    assert!(
        matches!(reader.signature, Some(roundhouse::ty::Ty::Fn { ref ret, .. })
            if matches!(**ret, roundhouse::ty::Ty::Bool)),
        "boolean attribute reader must be Bool-typed: {:?}",
        reader.signature
    );

    let writer = find("mod_note=").expect("writer synthesized");
    let body = format!("{:?}", writer.body);
    for falsey in ["\"0\"", "\"false\"", "\"f\""] {
        assert!(
            body.contains(falsey),
            "boolean cast must reject {falsey}:\n{body}"
        );
    }
    assert!(
        body.contains("to_s"),
        "cast compares via to_s (typed on every target):\n{body}"
    );
}
