//! A permit list whose keys are ALL non-scalar — `permit(widget_ids: [])`,
//! no bare symbol anywhere in the list.
//!
//! Every non-scalar key is already dropped from the synthesized params
//! record, with a `lower_residue` warning (see
//! `tests/params_nested_array_key.rs`). That drop is fine when SOME
//! scalar key survives it — the record still has fields, and its `[]`
//! reader is a `case` over them. But when NO key survives, `spec.fields`
//! is empty and the reader became
//!
//!   def [](key)
//!     case key
//!     end
//!   end
//!
//! an ARMLESS `case`, which is a Ruby syntax error (`unexpected 'end'`,
//! Prism: "expected at least one when or in clause after case"). Nothing
//! in the fixture corpus had a permit list this shape, so it went
//! unnoticed until the generic Widget/Gadget reduction here.
//!
//! Fix: no fields means the `[]` body is a bare `nil` literal instead of
//! a `Case` with zero arms — same signature, same `Ty::Untyped` result,
//! still parses, and callers already treat an absent key as nil-ish
//! (PRESENCE IS NOT HONORED on this reader; see its doc comment).

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

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widget_batches", force: :cascade do |t|
    t.string "name"
  end
end
"#;

const CONTROLLER: &str = r#"class WidgetBatchesController < ApplicationController
  def update
    @widget_batch = WidgetBatch.find(params[:id])
    @widget_batch.attributes = widget_batch_params.to_attrs
    @widget_batch.save
  end

  private

  def widget_batch_params
    params.require(:widget_batch).permit(widget_ids: [])
  end
end
"#;

fn lowered() -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/widget_batch.rb", "class WidgetBatch < ApplicationRecord\nend\n"),
        ("app/controllers/widget_batches_controller.rb", CONTROLLER),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted(suffix: &str) -> String {
    let app = lowered();
    ruby::emit_lowered_controllers(&app)
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

#[test]
fn a_permit_list_with_no_surviving_scalar_key_still_emits_a_params_record() {
    let params = emitted("app/models/widget_batch_params.rb");
    assert_parses(&params, "app/models/widget_batch_params.rb");
    assert!(
        !params.contains("widget_ids"),
        "the only (non-scalar) key has no slot in the record:\n{params}"
    );
}

/// The whole point: no armless `case` in the `[]` reader.
#[test]
fn the_index_reader_is_not_an_armless_case() {
    let params = emitted("app/models/widget_batch_params.rb");
    assert!(
        !params.contains("case key"),
        "an empty field list must not emit a `case` at all (no `when` clause to give it):\n{params}"
    );
    assert_parses(&params, "app/models/widget_batch_params.rb");
}

#[test]
fn dropping_the_only_key_still_files_a_residue_warning() {
    let (_files, diags) = roundhouse::emit::diagnostics::scope(|| {
        let app = lowered();
        ruby::emit_lowered_controllers(&app)
    });
    assert!(
        diags.iter().any(|d| d.message.contains("permitted key `widget_ids`")
            && d.message.contains("non-scalar")),
        "the dropped key is on the ledger, not silent: {:?}",
        diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>(),
    );
}
