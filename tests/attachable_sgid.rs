//! The `ActionText::Attachable` sgid round trip — mint and dereference.
//!
//! `Content#attachables` answered `[]` because dereferencing a signed
//! GlobalID needs a name-to-class map, and building one means
//! reflection or per-model registration at load. Neither is needed for
//! the shape app code actually writes: `attachables.grep(User)` has
//! already named its class, so `lower::attachables_grep` turns it into
//! a query over the ids `attachable_ids("User")` answers.
//!
//! The MINT is per-model with the name baked in — no `self.class.name`,
//! the same rule `lower::signed_id` states — and it lands only on
//! models that mix `ActionText::Attachable` in, which campfire does one
//! level down (`User` includes `Mentionable`, which includes it).

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
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

/// campfire's shape exactly: the marker is one level down, inside a
/// concern the model includes.
const MENTIONABLE: &str = r#"module User::Mentionable
  include ActionText::Attachable

  def to_attachable_partial_path
    "users/mention"
  end
end
"#;

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/user/mentionable.rb", MENTIONABLE),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include Mentionable\nend\n",
        ),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
    ]))
    .expect("ingest")
}

fn model_src(name: &str) -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(name))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {name}; got {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        })
}

#[test]
fn an_attachable_model_mints_its_sgid_with_the_name_baked_in() {
    let src = model_src("user.rb");
    assert!(
        src.contains(r#"ActionText::SignedGlobalId.generate("User", @id)"#),
        "the model name must be a literal, not a `self.class.name` read:\n{src}"
    );
}

/// The marker reaches `User` only through `User::Mentionable`; a model
/// that mixes nothing in must not answer.
#[test]
fn a_model_that_is_not_attachable_gets_nothing() {
    let src = model_src("room.rb");
    assert!(
        !src.contains("attachable_sgid"),
        "Room mixes in no Attachable:\n{src}"
    );
}

#[test]
fn grep_over_attachables_becomes_a_query_on_the_sgid_ids() {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/user/mentionable.rb", MENTIONABLE),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include Mentionable\nend\n",
        ),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  def mentioned\n    body.attachables.grep(User)\n  end\nend\n",
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_models(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room.rb"))
        .map(|f| f.content.clone())
        .expect("room.rb");
    assert!(
        src.contains(r#"attachable_ids("User")"#),
        "the grep's class must become the literal the ids are keyed on:\n{src}"
    );
    assert!(
        !src.contains(".grep("),
        "no grep may survive — nothing dereferences an sgid without a named class:\n{src}"
    );
}
