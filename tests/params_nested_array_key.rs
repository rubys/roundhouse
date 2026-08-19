//! A permit list with an ARRAY-VALUED key — `permit(:title, tags_a: [])`.
//!
//! lobsters' `StoriesController#story_params` is the case. Before, one
//! such key made the whole list unrecognizable: no `StoryParams` was
//! synthesized, the helper kept its source shape, and the emitted tree
//! called `@params.require(:story).permit(…)` — methods no target
//! defines. That is a `NoMethodError` under CRuby the moment the helper
//! runs, and it broke the Spinel AOT build of the lobsters benchmark
//! outright (the `.except(…)` chained off it dispatched to the only
//! `except` in the program, `HatRequestParams#except`).
//!
//! So the array-valued key is DROPPED and the rest of the list lowers.
//! Dropping is not free — a request may send `story[tags_a][]` and the
//! emitted app will not assign it, where Rails would — so it files a
//! `lower_residue` warning rather than passing silently. Carrying it
//! properly needs an `Array[String]` params field, which the record
//! (every slot `Ty::Str`, filled by `Params.str`) has no shape for yet.

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
  create_table "stories", force: :cascade do |t|
    t.string "title", null: false
    t.string "url"
    t.string "description"
    t.string "moderation_reason"
  end
end
"#;

const CONTROLLER: &str = r#"class StoriesController < ApplicationController
  def update
    @story = Story.find(params[:id])
    update_story_attributes
    @story.save
  end

  private

  def story_params
    p = params.require(:story).permit(
      :title, :url, :description, :moderation_reason,
      :tags_a => [],
    )

    if @user
      p
    else
      p.except(:moderation_reason)
    end
  end

  def update_story_attributes
    @story.attributes = story_params.except(:url)
  end
end
"#;

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/story.rb", "class Story < ApplicationRecord\nend\n"),
        ("app/controllers/stories_controller.rb", CONTROLLER),
    ]))
    .expect("ingest")
}

/// The pass under test is a POST-ANALYZE lowering, so the fixture goes
/// through the session rather than bare ingest.
fn lowered() -> roundhouse::App {
    let mut app = app();
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

#[test]
fn the_scalar_half_of_the_list_still_lowers() {
    let c = emitted("app/controllers/stories_controller.rb");
    assert!(
        c.contains("StoryParams.from_raw(@params)"),
        "the helper lowers to the synthesized record:\n{c}"
    );
    assert!(
        !c.contains("permit("),
        "no `require`/`permit` survives into the emit — nothing defines them:\n{c}"
    );
}

#[test]
fn the_array_valued_key_is_dropped_from_the_record() {
    let params = emitted("app/models/story_params.rb");
    for field in ["title", "url", "description", "moderation_reason"] {
        assert!(
            params.contains(&format!("def {field}")),
            "`{field}` is carried:\n{params}"
        );
    }
    assert!(
        !params.contains("tags_a"),
        "the array-valued key has no slot in a String-fielded record:\n{params}"
    );
}

#[test]
fn dropping_it_files_a_residue_warning() {
    let (_files, diags) = roundhouse::emit::diagnostics::scope(|| {
        let app = lowered();
        ruby::emit_lowered_controllers(&app)
    });
    assert!(
        diags.iter().any(|d| d.message.contains("`tags_a: []`")
            && d.message.contains("array-valued")),
        "the dropped key is on the ledger, not silent: {:?}",
        diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>(),
    );
}

/// `attributes=` takes the Symbol-keyed attribute hash, not a params
/// object — and this is the site that proves it, since nothing else in
/// lobsters assigns a params object through `attributes=`.
#[test]
fn attributes_assignment_converts_to_an_attribute_hash() {
    let c = emitted("app/controllers/stories_controller.rb");
    assert!(
        c.contains("@story.attributes = self.story_params.except(:url).to_attrs"),
        "`to_attrs` goes outside the `.except(…)` filter:\n{c}"
    );
}
