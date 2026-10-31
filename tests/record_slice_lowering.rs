//! What `lower::record_slice` makes of a typed `slice` site: the emitted
//! Hash, and the ledger entry for a site it leaves to the runtime.

use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "name"
    t.string "email"
  end
end
"#;

fn app(method: &str) -> roundhouse::App {
    let tree = [
        ("db/schema.rb", SCHEMA.to_string()),
        ("app/models/person.rb", format!("class Person < ApplicationRecord\n  {method}\nend\n")),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

/// The body of `probe`, as emitted after lowering.
fn emit(method: &str) -> String {
    let mut app = app(method);
    roundhouse::session::analyze_and_lower(&mut app);
    let out = emit_lowered_models(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    let at = out.find("def probe").unwrap_or_else(|| panic!("no probe:\n{out}"));
    let body = &out[at..];
    body[..body.find("\n  end\n").unwrap()].to_string()
}

fn slice_residue(method: &str, reason: &str) -> usize {
    let mut app = app(method);
    roundhouse::session::analyze_and_lower(&mut app)
        .iter()
        .filter(|d| matches!(&d.kind,
            roundhouse::diagnostic::DiagnosticKind::LowerResidue { construct, reason: r, .. }
                if construct.as_str() == "record-slice" && r.as_str() == reason))
        .count()
}

#[test]
fn a_name_given_twice_is_one_entry() {
    let body = emit("def probe = slice(:name, \"name\", [:email, :name])");
    assert_eq!(body.matches("\"name\" =>").count(), 1, "{body}");
    assert_eq!(body.matches("\"email\" =>").count(), 1, "{body}");
    assert!(body.find("\"name\"").unwrap() < body.find("\"email\"").unwrap(), "{body}");
}

#[test]
fn a_bare_slice_reads_through_self_past_a_local_of_the_same_name() {
    let body = emit("def probe(name) = slice(:name)");
    assert!(body.contains("\"name\" => self.name"), "{body}");
}

#[test]
fn names_known_only_at_run_time_are_ledgered() {
    assert_eq!(slice_residue("def probe(attr) = slice(attr)", "dynamic-names"), 1);
    assert_eq!(slice_residue("def probe = slice(:name)", "dynamic-names"), 0);
}
