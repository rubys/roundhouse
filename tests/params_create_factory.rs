//! `create` / `create!` handed a typed params object.
//!
//! Rails composes both as `new(attrs)` + save over an attribute HASH,
//! and the runtime keeps that shape. Once a controller's
//! `<resource>_params` helper returns a typed `<Resource>Params`, the
//! call site hands that object to a method that indexes it like a Hash —
//! `User.create!(user_params)` reached `initialize(attrs)` and asked a
//! class with no `[]` for `attrs[:name]`.
//!
//! `.new` was already rewritten to the typed `from_params` factory;
//! `create` is that factory plus the save it composes to. Two receiver
//! shapes, one defect:
//!
//! ```ruby
//! User.create!(user_params)              # → User.create_from_params!(...)
//! @user.notes.create!(note_params)       # → Note.from_params + fk + save!
//! ```

use roundhouse::app::App;
use roundhouse::dialect::LibraryClass;

fn app_from(files: Vec<(&str, &str)>) -> App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\n  \
    create_table :notes do |t|\n    t.string :body\n    t.integer :user_id\n  end\nend\n";

const MODELS: &[(&str, &str)] = &[
    ("app/models/user.rb", "class User < ApplicationRecord\n  has_many :notes\nend\n"),
    ("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :user\nend\n"),
];

fn app_with(controllers: Vec<(&str, &str)>) -> App {
    let mut files: Vec<(&str, &str)> = vec![("db/schema.rb", SCHEMA)];
    files.extend_from_slice(MODELS);
    files.extend(controllers);
    app_from(files)
}

fn model_lcs(app: &App) -> Vec<LibraryClass> {
    let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    roundhouse::lower::lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        vec![],
        &specs,
    )
    .0
}

fn method_names(lcs: &[LibraryClass], class: &str) -> Vec<String> {
    lcs.iter()
        .find(|lc| lc.name.0.as_str() == class)
        .unwrap_or_else(|| panic!("{class} lowered"))
        .methods
        .iter()
        .map(|m| m.name.as_str().to_string())
        .collect()
}

fn action_body(app: &App, controller: &str, action: &str) -> String {
    let lcs = roundhouse::lower::lower_controllers_to_library_classes(&app.controllers, Vec::new());
    let lc = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == controller)
        .unwrap_or_else(|| panic!("{controller} lowered"));
    let m = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == action)
        .unwrap_or_else(|| panic!("{controller}#{action}"));
    format!("{:?}", m.body)
}

const USERS_CREATE_BANG: (&str, &str) = (
    "app/controllers/users_controller.rb",
    r#"class UsersController < ApplicationController
  def create
    @user = User.create!(user_params)
  end

  private
    def user_params
      params.require(:user).permit(:name)
    end
end
"#,
);

#[test]
fn model_create_bang_goes_through_the_typed_factory() {
    let app = app_with(vec![USERS_CREATE_BANG]);
    let body = action_body(&app, "UsersController", "create");
    assert!(
        body.contains("create_from_params!"),
        "expected the typed create factory, got {body}"
    );

    // And the model actually has it — a rewrite naming a method nobody
    // synthesizes is the same NoMethodError one layer down.
    let names = method_names(&model_lcs(&app), "User");
    assert!(names.contains(&"create_from_params!".to_string()), "got {names:?}");
    assert!(names.contains(&"from_params".to_string()), "the factory it wraps");
}

#[test]
fn only_the_spelling_a_call_site_uses_is_synthesized() {
    // Demand-gated like `wants_except`: the runtime's `create` takes an
    // attribute Hash, so a model nobody creates this way must not grow
    // two methods on every target.
    let app = app_with(vec![USERS_CREATE_BANG]);
    let lcs = model_lcs(&app);
    let names = method_names(&lcs, "User");
    assert!(
        !names.contains(&"create_from_params".to_string()),
        "no call site uses the non-bang spelling: {names:?}"
    );
    // `Note` is permitted by nothing here, so it gets neither.
    let note = method_names(&lcs, "Note");
    assert!(
        !note.iter().any(|n| n.starts_with("create_from_")),
        "Note has no permit list at all: {note:?}"
    );
}

#[test]
fn a_receiver_that_is_not_the_lists_own_model_is_left_alone() {
    // `Note.create!(user_params)` has no typed factory to reach —
    // `Note.from_params` is sized to `:note`, not `:user`. Rewriting it
    // would name a method that doesn't exist.
    let app = app_with(vec![(
        "app/controllers/users_controller.rb",
        r#"class UsersController < ApplicationController
  def create
    @note = Note.create!(user_params)
  end

  private
    def user_params
      params.require(:user).permit(:name)
    end
end
"#,
    )]);
    let body = action_body(&app, "UsersController", "create");
    assert!(
        !body.contains("create_from_params"),
        "off-resource receiver must not be rewritten, got {body}"
    );
    let names = method_names(&model_lcs(&app), "User");
    assert!(
        !names.iter().any(|n| n.starts_with("create_from_")),
        "and nothing is synthesized for it: {names:?}"
    );
}

#[test]
fn association_create_expands_to_factory_plus_foreign_key_plus_save() {
    let app = app_with(vec![(
        "app/controllers/notes_controller.rb",
        r#"class NotesController < ApplicationController
  before_action :set_user

  def create
    @note = @user.notes.create!(note_params)
  end

  private
    def set_user
      @user = User.find(params[:user_id])
    end

    def note_params
      params.require(:note).permit(:body)
    end
end
"#,
    )]);
    let body = action_body(&app, "NotesController", "create");
    // `create` on an association is `build` + save; the build half was
    // already typed, so this is that expansion with the save restored.
    assert!(body.contains("from_params"), "typed factory: {body}");
    assert!(body.contains("user_id="), "the association's foreign key: {body}");
    assert!(body.contains("save!"), "the save `create!` composes to: {body}");
    // The FK comes from the parent, so a request-supplied `user_id`
    // can't retarget the row — and `:user_id` was never permitted.
    let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    let spec = specs
        .canonical(&roundhouse::Symbol::from("note"))
        .expect("a :note spec");
    assert_eq!(
        spec.fields.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
        vec!["body"]
    );
}

#[test]
fn an_attribute_hash_create_is_untouched() {
    // Only the typed arm is recognized — a Hash `create` already works
    // through the runtime, and restating it here would be churn.
    let app = app_with(vec![(
        "app/controllers/notes_controller.rb",
        r#"class NotesController < ApplicationController
  def create
    @user = User.find(params[:user_id])
    @note = @user.notes.create!(body: "hi")
  end

  private
    def note_params
      params.require(:note).permit(:body)
    end
end
"#,
    )]);
    let body = action_body(&app, "NotesController", "create");
    assert!(
        body.contains("create!") && !body.contains("from_params"),
        "hash-form create should flow through unchanged, got {body}"
    );
}

/// The residual case the typed factories above do NOT cover: the params
/// object handed to a USER-WRITTEN method whose parameter is untyped, so
/// there is nothing to specialize against.
///
/// campfire:
///
/// ```ruby
/// # controller
/// Rooms::Open.create_for(room_params, users: Current.user)
/// # model
/// def create_for(attributes, users:)
///   create!(attributes).tap { |room| room.memberships.grant_to users }
/// end
/// ```
///
/// `create_for` cannot be specialized — `Rooms::Direct` calls the same
/// method with a `{}` literal — so `create!` receives the params object
/// and its synthesized `initialize` asks for `attrs[:name]`. The params
/// class answers `[]` for exactly this.
#[test]
fn a_params_class_answers_the_hash_index() {
    let app = app_with(vec![(
        "app/controllers/users_controller.rb",
        r#"class UsersController < ApplicationController
  def create
    User.create_for(user_params)
  end

  private
    def user_params
      params.require(:user).permit(:name)
    end
end
"#,
    )]);
    let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    let lcs = roundhouse::lower::controller_to_library::params::synthesize_params_classes(&specs);
    let names = method_names(&lcs, "UserParams");
    assert!(names.iter().any(|n| n == "[]"), "no [] on UserParams: {names:?}");
}

/// The index reads the same slot `to_h` does — the two Hash faces of the
/// object agree. Presence is NOT gated here, which is a known gap
/// (documented at the synthesizer): an absent key reads as the `""` the
/// slot was initialized to, and gating it needs a Rust-emitter fix
/// first, since an `if`-with-nil-else in an arm body is E0317 in
/// expression position. This test pins the current shape so that fix has
/// something to change deliberately.
#[test]
fn the_hash_index_reads_the_same_slot_as_to_h() {
    let app = app_with(vec![(
        "app/controllers/users_controller.rb",
        r#"class UsersController < ApplicationController
  def create
    User.create_for(user_params)
  end

  private
    def user_params
      params.require(:user).permit(:name)
    end
end
"#,
    )]);
    let specs = roundhouse::lower::controller_to_library::params::collect_specs(&app.controllers);
    let lcs = roundhouse::lower::controller_to_library::params::synthesize_params_classes(&specs);
    let lc = lcs.iter().find(|lc| lc.name.0.as_str() == "UserParams").expect("UserParams");
    let m = lc.methods.iter().find(|m| m.name.as_str() == "[]").expect("[]");
    let body = format!("{:?}", m.body);
    // One arm per permitted field, reading that field's slot.
    assert!(body.contains("name"), "{body}");
    assert!(!body.contains("name_provided"), "presence gated — update the note: {body}");
}
