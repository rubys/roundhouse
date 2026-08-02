//! `lower::as_json_writer` — turning an `as_json` pair list into a
//! straight-line JSON writer.
//!
//! Assertions are on the EMITTED RUBY rather than on IR node shapes:
//! the whole point of the pass is the text a target compiles, and a
//! misplaced comma or a wrong encoder is invisible in an IR assertion
//! but obvious here.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::as_json_shape::as_json_pairs_for_no_arg_call;
use roundhouse::lower::as_json_writer::writer_body;

fn app_from(files: Vec<(&str, &str)>) -> roundhouse::App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

/// Recognize `<model>#as_json`, build its writer, and render the body
/// as Ruby.
fn writer_ruby(app: &roundhouse::App, model_name: &str, assocs: &[&str]) -> String {
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == model_name)
        .unwrap_or_else(|| panic!("no model {model_name}"));
    let (params, body) = model
        .body
        .iter()
        .find_map(|item| match item {
            roundhouse::dialect::ModelBodyItem::Method { method, .. }
                if method.name.as_str() == "as_json" =>
            {
                Some((method.params.clone(), method.body.clone()))
            }
            _ => None,
        })
        .expect("as_json");
    let pairs = as_json_pairs_for_no_arg_call(&params, &body).expect("recognized");
    let table = app
        .schema
        .tables
        .iter()
        .find(|(n, _)| n.as_str() == table_name(model_name))
        .map(|(_, t)| t);
    let assoc_syms: Vec<roundhouse::ident::Symbol> = assocs
        .iter()
        .map(|a| roundhouse::ident::Symbol::from(*a))
        .collect();
    let stmts = writer_body(&pairs, table, &assoc_syms).expect("writer built");
    stmts
        .iter()
        .map(roundhouse::emit::ruby::emit_expr)
        .collect::<Vec<_>>()
        .join("\n")
}

fn table_name(model: &str) -> String {
    format!("{}s", model.to_lowercase())
}

const SCHEMA: &str = "ActiveRecord::Schema.define do\n\
                      \x20 create_table :widgets do |t|\n\
                      \x20   t.string :short_id\n\
                      \x20   t.integer :score\n\
                      \x20   t.datetime :created_at\n\
                      \x20 end\n\
                      end\n";

#[test]
fn unconditional_pairs_render_as_a_flat_accumulator_with_static_commas() {
    let app = app_from(vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n\
             \x20 def as_json(_options = {})\n\
             \x20   h = [ :short_id, :score ]\n\
             \x20   js = {}\n\
             \x20   h.each do |k|\n\
             \x20     js[k] = self.send(k)\n\
             \x20   end\n\
             \x20   js\n\
             \x20 end\n\
             end\n",
        ),
    ]);
    let ruby = writer_ruby(&app, "Widget", &[]);

    // Opens the accumulator, brace, then one pair per key. The FIRST
    // key carries no comma; the second does.
    assert!(ruby.contains("io = String.new"), "got:\n{ruby}");
    assert!(ruby.contains(r#"io << "{""#), "got:\n{ruby}");
    assert!(ruby.contains(r#"io << "\"short_id\":""#), "got:\n{ruby}");
    assert!(ruby.contains(r#"io << ",\"score\":""#), "leading comma on 2nd key:\n{ruby}");
    assert!(!ruby.contains(r#"io << ",\"short_id\":""#), "1st key must not lead with a comma:\n{ruby}");
    assert!(ruby.contains("JsonBuilder.encode_value(self.short_id)"), "got:\n{ruby}");
    assert!(ruby.trim_end().ends_with("io"), "returns the accumulator:\n{ruby}");
}

#[test]
fn a_temporal_column_routes_through_encode_datetime_and_the_raw_reader() {
    // Same choice the jbuilder lowerer makes: serialize from the stored
    // ISO-8601 text, not the parsing reader — the reformat is exact and
    // skips a parse/format round-trip per row.
    let app = app_from(vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n\
             \x20 def as_json(_options = {})\n\
             \x20   h = [ :created_at ]\n\
             \x20   js = {}\n\
             \x20   h.each do |k|\n\
             \x20     js[k] = self.send(k)\n\
             \x20   end\n\
             \x20   js\n\
             \x20 end\n\
             end\n",
        ),
    ]);
    let ruby = writer_ruby(&app, "Widget", &[]);
    assert!(
        ruby.contains("JsonBuilder.encode_datetime(self.created_at_raw)"),
        "got:\n{ruby}"
    );
    assert!(!ruby.contains("encode_value(self.created_at)"), "got:\n{ruby}");
}

#[test]
fn a_conditional_key_carries_its_comma_inside_the_guard() {
    // The separator has to be skipped with the key it separates, or a
    // skipped key leaves a dangling comma and the JSON is malformed.
    let app = app_from(vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n\
             \x20 def as_json(_options = {})\n\
             \x20   attrs = [ :short_id ]\n\
             \x20   attrs.push :score if !self.is_admin?\n\
             \x20   h = super(:only => attrs)\n\
             \x20   h\n\
             \x20 end\n\
             end\n",
        ),
    ]);
    let ruby = writer_ruby(&app, "Widget", &[]);

    // The guarded key's append sits inside an `if`, and the comma is
    // part of that same guarded append.
    assert!(ruby.contains("if"), "expected a guard:\n{ruby}");
    let guarded = ruby
        .split("if ")
        .nth(1)
        .unwrap_or_else(|| panic!("no guard body:\n{ruby}"));
    assert!(
        guarded.contains(r#"io << ",\"score\":""#),
        "comma must ride inside the guard:\n{ruby}"
    );
}

#[test]
fn a_key_that_serializes_an_associated_record_is_declined() {
    // `encode_value` would fall back to `to_s` and quote it — valid
    // JSON, wrong data. Nested records need their own writer, which is
    // not modeled yet, so decline loudly instead.
    let app = app_from(vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n\
             \x20 def as_json(_options = {})\n\
             \x20   h = [ :short_id, { :submitter_user => :user } ]\n\
             \x20   js = {}\n\
             \x20   h.each do |k|\n\
             \x20     js[k] = self.send(k)\n\
             \x20   end\n\
             \x20   js\n\
             \x20 end\n\
             end\n",
        ),
    ]);
    let model = app.models.iter().find(|m| m.name.0.as_str() == "Widget").unwrap();
    let (params, body) = model
        .body
        .iter()
        .find_map(|item| match item {
            roundhouse::dialect::ModelBodyItem::Method { method, .. }
                if method.name.as_str() == "as_json" =>
            {
                Some((method.params.clone(), method.body.clone()))
            }
            _ => None,
        })
        .unwrap();
    let pairs = as_json_pairs_for_no_arg_call(&params, &body).expect("recognized");
    let err = writer_body(&pairs, None, &[roundhouse::ident::Symbol::from("user")])
        .expect_err("declined");
    assert!(err.contains("associated record"), "got: {err}");
}
