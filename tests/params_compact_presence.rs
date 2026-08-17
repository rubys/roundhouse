//! `<permit-chain>.compact`, and the conflation it exposed.
//!
//! MEASURED against Rails 8.1 (`ActionController::Parameters#compact`):
//! it drops keys whose value is explicitly nil and KEEPS `""`. So for a
//! form POST it changes nothing at all — what it really expresses is
//! "assign only what the request actually provided", which is what
//! `permit` + `update` already do in Rails, because an absent key simply
//! isn't in the hash.
//!
//! Our `from_raw` couldn't say that: it collapsed an ABSENT key to `""`,
//! which `update` then assigned, clobbering the column. campfire's
//! profile page has an avatar-only form, so submitting it blanked the
//! user's name, email and bio. `.compact` was NoMethodError on top of
//! that, which is what made the bug visible.
//!
//! Fix: EVERY params class is presence-aware — a `<field>_provided` Bool
//! slot beside each value slot, which the TYPED assignment methods
//! (`update_from_<class>` / `…!` / `from_params`) guard on.
//! (Plain `update` / `update!` take an attribute Hash, Rails' own
//! contract, and answer the same question with `attrs.key?` — a Hash can
//! simply omit a key, which is the fact a params object needs a separate
//! slot to record.) Unconditional, because the conflation was
//! never specific to `.compact`: any `@record.update(<x>_params)` on a
//! form that omits a field assigned `""` where Rails assigns nothing.
//! `.compact` is then just dropped from the chain.
//!
//! Presence is a different fact from value, so it gets its own slot. Nilable slots were tried first and cost
//! more: `if !p.name.nil? { self.name = p.name }` needs the emitter to
//! flow-narrow an Option through the guard, which rust2 doesn't do (6
//! fresh `cargo check` errors, measured) and every other strict target
//! would need too. A Bool beside a String needs nothing from any emitter.

use roundhouse::app::App;
use roundhouse::dialect::{LibraryClass, LibraryClassOrigin};

fn app_from(files: Vec<(&str, &str)>) -> App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n    t.string :bio\n  end\nend\n";

fn app_with(tail: &str) -> App {
    let controller = format!(
        r#"class UsersController < ApplicationController
  def update
    @user = User.find(params[:id])
    @user.update(user_params)
  end

  private
    def user_params
      params.require(:user).permit(:name, :bio){tail}
    end
end
"#
    );
    app_from(vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "app/controllers/users_controller.rb",
            Box::leak(controller.into_boxed_str()),
        ),
    ])
}

fn params_class(app: &App) -> LibraryClass {
    roundhouse::lower::lower_controllers_to_library_classes(&app.controllers, Vec::new())
        .into_iter()
        .find(|lc| matches!(lc.origin, Some(LibraryClassOrigin::ResourceParams { .. })))
        .expect("a synthesized params class")
}

fn model_method(app: &App, name: &str) -> String {
    let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    let (lcs, _) = roundhouse::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        vec![],
        &specs,
    );
    let user = lcs.iter().find(|lc| lc.name.0.as_str() == "User").expect("User lowered");
    let m = user
        .methods
        .iter()
        .find(|m| m.name.as_str() == name)
        .unwrap_or_else(|| panic!("User#{name}"));
    format!("{:?}", m.body)
}

#[test]
fn compact_makes_the_class_presence_aware() {
    let app = app_with(".compact");
    let lc = params_class(&app);
    let names: Vec<&str> = lc.methods.iter().map(|m| m.name.as_str()).collect();
    for slot in ["name_provided", "bio_provided"] {
        assert!(names.contains(&slot), "expected a {slot} reader, got {names:?}");
        assert!(
            names.contains(&format!("{slot}=").as_str()),
            "and its writer, got {names:?}"
        );
    }
}

#[test]
fn the_value_slots_stay_plain_strings() {
    // The whole point of a separate presence slot: no Option, so no
    // emitter has to flow-narrow one through the `update` guard.
    let app = app_with(".compact");
    let lc = params_class(&app);
    let reader = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "name")
        .expect("name reader");
    let sig = format!("{:?}", reader.signature.as_ref().expect("signature"));
    assert!(
        !sig.contains("Nil") && sig.contains("Str"),
        "value slot must not widen to nilable: {sig}"
    );
}

#[test]
fn update_guards_on_provided_instead_of_assigning_blanks() {
    let app = app_with(".compact");
    // The TYPED update — the one a controller's `@user.update(user_params)`
    // is rewritten to. Plain `update` takes an attribute Hash (Rails'
    // own contract) and answers the same question with `attrs.key?`;
    // `<field>_provided` is what a params object carries instead, since
    // its slots always exist.
    let body = model_method(&app, "update_from_user_params");
    assert!(body.contains("name_provided"), "expected a presence guard: {body}");
    assert!(body.contains("bio_provided"), "expected a presence guard: {body}");
    // `from_params` needs the same guard — Rails' `new(attrs)` never
    // sees an absent key either.
    let body = model_method(&app, "from_params");
    assert!(body.contains("name_provided"), "expected a presence guard: {body}");
}

#[test]
fn compact_is_dropped_from_the_chain() {
    // Rails' `compact` removes nil-valued keys; a presence-aware
    // `from_raw` already reads "not provided" as its own flag, so there
    // is nothing left to remove. Emitting an identity method would be
    // the same no-op with a call in front of it.
    let app = app_with(".compact");
    let lcs = roundhouse::lower::lower_controllers_to_library_classes(&app.controllers, Vec::new());
    let ctrl = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "UsersController")
        .expect("UsersController");
    let helper = ctrl
        .methods
        .iter()
        .find(|m| m.name.as_str() == "user_params")
        .expect("user_params");
    let body = format!("{:?}", helper.body);
    assert!(body.contains("from_raw"), "still the typed factory: {body}");
    assert!(
        !body.contains("compact"),
        "the compact call must be gone — the params class has no such method: {body}"
    );
}

#[test]
fn presence_tracking_does_not_wait_for_compact() {
    // The conflation was never specific to `.compact`: ANY
    // `@record.update(<x>_params)` against a form that omits a field
    // assigned `""` where Rails assigns nothing. So a chain nobody
    // compacts is presence-aware too.
    let app = app_with("");
    let lc = params_class(&app);
    let names: Vec<&str> = lc.methods.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"name_provided"), "got {names:?}");
    assert!(names.contains(&"bio_provided"), "got {names:?}");
    let body = model_method(&app, "update_from_user_params");
    assert!(
        body.contains("name_provided") && !body.contains("nil?"),
        "the presence flag replaces the nil convention outright: {body}"
    );
}

#[test]
fn a_merged_server_side_value_counts_as_provided() {
    // `permit(...).merge(k: v)` folds in a value the REQUEST never
    // carried, so `update` has to assign it. The `<field>=` writer can't
    // set the flag itself — emitters collapse an AttributeWriter into a
    // plain field and drop its body — so the merge expansion sets it.
    let app = app_from(vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "app/controllers/users_controller.rb",
            r#"class UsersController < ApplicationController
  def update
    @user = User.find(params[:id])
    @user.update(user_params)
  end

  private
    def user_params
      params.require(:user).permit(:name).merge(bio: "set by the server")
    end
end
"#,
        ),
    ]);
    let lcs = roundhouse::lower::lower_controllers_to_library_classes(&app.controllers, Vec::new());
    let helper = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "UsersController")
        .expect("UsersController")
        .methods
        .iter()
        .find(|m| m.name.as_str() == "user_params")
        .expect("user_params");
    let body = format!("{:?}", helper.body);
    assert!(
        body.contains("bio_provided"),
        "a merged key must be marked provided or `update` skips it: {body}"
    );
}
