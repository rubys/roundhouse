//! `has_one` builders (`lower::apply_has_one_builder_lowering`).
//!
//! Rails generates `build_x` / `create_x` / `create_x!` alongside a
//! `has_one :x` reader. campfire calls `create_webhook!` twice — once on
//! an explicit receiver and once on implicit self — and before this pass
//! the emitted tree CALLED the name with nothing defining it, so both
//! sites were a NameError waiting to happen (and one of them was a
//! strict-emit error).
//!
//! These are shape tests over the rewrite: the call becomes an ordinary
//! `Target.create!` with the foreign key filled in, which is a shape the
//! AR catalog already types and every target already emits.

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::{emit_library, emit_lowered_models};
use roundhouse::ingest::{ingest_library_classes, ingest_model, ingest_schema};
use roundhouse::lower::apply_has_one_builder_lowering;
use roundhouse::App;

/// A `User has_one :webhook` app plus the given library-class source,
/// analyzed and run through the pass.
fn lower_and_emit(source: &str) -> String {
    lower_and_emit_with_user_body("", source)
}

/// Same, with extra method source spliced into `User`'s own body — the
/// only place an implicit-self builder call can resolve its owner.
fn lower_and_emit_with_user_body(user_extra: &str, source: &str) -> String {
    let schema = ingest_schema(
        br#"
ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "users", force: :cascade do |t|
    t.string "name"
  end

  create_table "webhooks", force: :cascade do |t|
    t.string "url"
    t.integer "user_id"
  end
end
"#,
        "db/schema.rb",
    )
    .expect("ingest schema");
    let mut app = App::new();
    for (src, path) in [
        (
            &format!(
                "class User < ApplicationRecord\n  has_one :webhook, dependent: :delete\n{user_extra}end\n"
            ),
            "app/models/user.rb",
        ),
        (
            &"class Webhook < ApplicationRecord\n  belongs_to :user\nend\n".to_string(),
            "app/models/webhook.rb",
        ),
    ] {
        let model = ingest_model(src.as_bytes(), path, &schema, &Default::default())
            .expect("ingest model")
            .expect("model recognized");
        app.models.push(model);
    }
    app.schema = schema;

    let classes =
        ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    apply_has_one_builder_lowering(&mut app);
    // Models AND library classes: the explicit-receiver sites live in a
    // library class, the implicit-self one in the model's own body.
    emit_library(&app)
        .into_iter()
        .chain(emit_lowered_models(&app))
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn explicit_receiver_becomes_a_create_with_the_foreign_key() {
    // campfire's `User::Bot.create_bot!` shape: the owner is a block
    // parameter, so the foreign key has to be read off it at the call
    // site rather than from an ivar.
    let out = lower_and_emit(
        r#"
class BotMaker
  def make(id, url)
    user = User.find(id)
    user.create_webhook!(url: url)
  end
end
"#,
    );
    assert!(
        !out.contains("create_webhook!"),
        "the builder name must not survive — nothing defines it:\n{out}",
    );
    assert!(
        out.contains("Webhook.create!") && out.contains("user_id: user.id"),
        "expected `Webhook.create!(user_id: user.id, url: url)`:\n{out}",
    );
    assert!(out.contains("url: url"), "caller kwargs are carried through:\n{out}");
}

#[test]
fn the_bang_and_non_bang_and_build_forms_pick_different_constructors() {
    let out = lower_and_emit(
        r#"
class BotMaker
  def bang(id, url)
    User.find(id).create_webhook!(url: url)
  end

  def plain(id, url)
    User.find(id).create_webhook(url: url)
  end

  def unsaved(id, url)
    User.find(id).build_webhook(url: url)
  end
end
"#,
    );
    // `create_x!` also starts with the `create_` prefix, so a pass that
    // tested the shorter name first would strip the `!` and build the
    // non-bang constructor for both.
    assert!(out.contains("Webhook.create!"), "bang form → create!:\n{out}");
    assert!(
        out.contains("Webhook.create(") || out.contains("Webhook.create ("),
        "non-bang form → create:\n{out}",
    );
    assert!(out.contains("Webhook.new"), "build form → new:\n{out}");
}

#[test]
fn a_name_that_is_not_an_association_is_left_alone() {
    // The prefix match must not swallow an ordinary method that happens
    // to start with `create_` or `build_`.
    let out = lower_and_emit(
        r#"
class BotMaker
  def make(id)
    User.find(id).create_something_else(x: 1)
  end
end
"#,
    );
    assert!(
        out.contains("create_something_else"),
        "a non-association `create_*` keeps its call:\n{out}",
    );
    assert!(!out.contains("Webhook.create"), "and builds nothing:\n{out}");
}

/// The form that a receiver-only rewrite MISSES, and did: campfire's
/// `User::Bot#update_webhook_url!` calls `create_webhook!(url: url)` on
/// implicit self. There is no receiver to take a type from, so the owner
/// comes from the enclosing model — here `User`'s own body. Left
/// unhandled, this site emitted a call to a name nothing defines.
#[test]
fn implicit_self_resolves_its_owner_from_the_enclosing_model() {
    let out = lower_and_emit_with_user_body(
        r#"
  def set_webhook!(url)
    create_webhook!(url: url)
  end
"#,
        "class Unused\nend\n",
    );
    assert!(
        !out.contains("create_webhook!"),
        "the bare builder call must be rewritten too:\n{out}",
    );
    assert!(
        out.contains("Webhook.create!") && out.contains("user_id: @id"),
        "on implicit self the foreign key comes from `@id`, the way the \
         synthesized readers read it:\n{out}",
    );
}
