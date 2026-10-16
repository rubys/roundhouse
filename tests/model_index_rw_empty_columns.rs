//! A table with NO columns at all — `create_table "widgets", id: false do
//! |t| end` — backing an otherwise ordinary model.
//!
//! `model_to_library::schema::synth_index_read` and `synth_index_write`
//! are modeled on the controller-side `params::synth_index_read` (same
//! `Case`-over-Symbol-literals shape; see
//! `tests/params_all_non_scalar_keys.rs` for that one's bug). Both have
//! the identical gap: zero columns means zero `case` arms, and
//! `[](name); case name; end; end` / `[]=(name, value); case name; end;
//! end` are armless `case`s — `unexpected 'end'` under CRuby, "expected
//! at least one when or in clause after case" under Prism.
//!
//! Degenerate (nothing stores an `id: false`, column-free table on
//! purpose), but reachable — nothing in schema ingest requires a
//! `create_table` block to declare any column — so both get the same
//! nil-body guard as the params-record reader. With both guarded, the
//! WHOLE emitted model file parses, not just the reader in isolation.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"widgets\", id: false, force: :cascade do |t|\n  end\nend\n";

fn lowered() -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted(suffix: &str) -> String {
    let app = lowered();
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .expect("emitted file")
}

/// `source` parses as Ruby; the message names `path`.
fn assert_parses(source: &str, path: &str) {
    let result = ruby_prism::parse(source.as_bytes());
    let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
    assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
}

/// The body right after `def <open>` — stops at the method's own `end`.
fn method_body<'a>(model: &'a str, open: &str) -> &'a str {
    let start = model.find(open).unwrap_or_else(|| panic!("no `{open}`:\n{model}")) + open.len();
    let end = model[start..]
        .find("\n  end\n")
        .unwrap_or_else(|| panic!("no closing `end` for `{open}`"));
    &model[start..start + end]
}

#[test]
fn a_column_free_table_still_emits_a_parseable_index_reader_and_writer() {
    let model = emitted("app/models/widget.rb");

    let read_body = method_body(&model, "def [](name)\n");
    assert_eq!(
        read_body.trim(),
        "nil",
        "an empty column list must not emit a `case` at all (no `when` clause to give it):\n{model}"
    );
    let write_body = method_body(&model, "def []=(name, value)\n");
    assert_eq!(
        write_body.trim(),
        "nil",
        "same gap on the writer — no `when` clause to give its `case` either:\n{model}"
    );

    // The whole point: with BOTH guarded, the entire model file parses,
    // not just one method pulled out in isolation.
    assert_parses(&model, "app/models/widget.rb");
}
