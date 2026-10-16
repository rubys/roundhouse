use roundhouse::emit::ruby::emit_lowered_models;
use roundhouse::ingest::ingest_app_from_tree;

fn emit(body: &str) -> String {
    let tree = [(
        "app/models/thing.rb",
        format!("class Thing < ApplicationRecord\n  def probe(flag)\n    {body}\n  end\nend\n"),
    )]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let out = emit_lowered_models(&app).into_iter().map(|f| f.content).collect::<Vec<_>>().join("\n");
    let at = out.find("def probe(flag)\n").unwrap_or_else(|| panic!("no probe:\n{out}"));
    let body = &out[at + "def probe(flag)\n".len()..];
    body[..body.find("\n  end\n").unwrap()].lines().map(str::trim).collect::<Vec<_>>().join(" ")
}

#[test]
fn write_headers_becomes_the_first_row() {
    assert_eq!(
        emit("CSV.generate(\"\", headers: [\"a\"], write_headers: true) { |csv| csv << [1] }"),
        "CSV.generate(\"\") { |csv| csv << [\"a\"] csv << [1] }"
    );
}

/// A block with an optional parameter is not the `|csv|` shape: the call
/// is left as written, its `headers:` / `write_headers:` options included.
#[test]
fn a_block_with_an_optional_parameter_keeps_the_call_as_written() {
    assert_eq!(
        emit("CSV.generate(\"\", headers: [\"a\"], write_headers: true) { |csv, row = [1]| csv << row }"),
        "CSV.generate(\"\", headers: [\"a\"], write_headers: true) { |csv, row = [1]| csv << row }"
    );
}

#[test]
fn a_computed_write_headers_guards_the_row() {
    assert_eq!(
        emit("CSV.generate(headers: [\"a\"], write_headers: flag) { |csv| csv << [1] }"),
        "CSV.generate { |csv| csv << [\"a\"] if flag csv << [1] }"
    );
}

#[test]
fn headers_without_write_headers_are_only_dropped() {
    assert_eq!(
        emit("CSV.generate(headers: [\"a\"], col_sep: \";\") { |csv| csv << [1] }"),
        "CSV.generate(col_sep: \";\") { |csv| csv << [1] }"
    );
}
