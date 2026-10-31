//! A scope whose lambda takes a required parameter after optional ones
//! (`->(column = "price", op = ">=", amount)`, Prism's `posts`) refuses
//! the app at ingest, naming the scope, instead of emitting it without
//! that parameter. Ingest dropped it: the class method the scope becomes
//! had no slot for it, the body read an unbound name, and the caller's
//! last argument landed in an optional, so `Gadget.priced(:price, 6)`
//! bound `op = 6` and answered `none`. Ruby binds a trailing required
//! parameter from the end of the argument list.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"gadgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.integer \"price\"\n  end\nend\n";

fn gadget(scope: &str) -> String {
    format!("class Gadget < ApplicationRecord\n  OPS = %w[> < >= <=].freeze\n{scope}end\n")
}

fn ingest(model: &str) -> Result<roundhouse::App, String> {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/gadget.rb", model),
    ]))
    .map_err(|e| format!("{e:?}"))
}

#[test]
fn a_required_scope_parameter_after_optional_ones_is_refused() {
    let scope = "  scope :priced, ->(column = \"price\", op = \">=\", amount) {\n    OPS.include?(op) ? where(\"#{column} #{op} :min\", min: amount || -1) : none\n  }\n";
    let err = ingest(&gadget(scope)).expect_err("a trailing required scope parameter is refused");
    assert!(err.contains("scope :priced: a required parameter after optional ones is not supported"), "{err}");
}

#[test]
fn optional_and_leading_required_scope_parameters_still_ingest() {
    let scope = "  scope :priced, ->(amount, column = \"price\", op = \">=\") {\n    OPS.include?(op) ? where(\"#{column} #{op} :min\", min: amount) : none\n  }\n";
    ingest(&gadget(scope)).expect("leading required and trailing optional parameters ingest");
}
