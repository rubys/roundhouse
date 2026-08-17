//! One resource, several permit lists.
//!
//! Strong params are recognized per call site, so an app that permits
//! the same resource differently in different controllers declares
//! several distinct mass-assignment boundaries. Keying the synthesized
//! `<Resource>Params` classes by resource ALONE collapsed them to one
//! (first controller wins) — campfire's `Accounts::BotsController` won
//! the `:user` slot and the first-run signup silently lost
//! `email_address` / `password`, so a newly created account had no
//! credentials.
//!
//! Taking the union instead is not a fix: it would put `password` on
//! the bots controller's permitted set, which is exactly the
//! mass-assignment Rails' `permit` exists to prevent.
//!
//! So: one class per distinct `(resource, fields)` pair. The controller
//! the resource is named for keeps the unqualified CLASS name (and with
//! it the model's plain `from_params` factory); the others are qualified
//! by their declaring controller. The typed ASSIGNMENT method is named
//! for its permit list in every case — `update` / `update!` are Rails'
//! attribute-Hash contract over the model's whole writable surface, a
//! different and wider thing than any one mass-assignment boundary.

use roundhouse::dialect::LibraryClassOrigin;
use roundhouse::ident::Symbol;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::controller_to_library::params::collect_specs;
use roundhouse::lower::{lower_controllers_to_library_classes, lower_models_with_registry_and_params};

/// A campfire-shaped app: four `:user` permit lists across four
/// controllers, three of them distinct.
fn campfire_like_app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :name\n    t.string :email_address\n    t.string :bio\n    t.string :webhook_url\n  end\nend\n",
        ),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "app/controllers/users_controller.rb",
            r#"class UsersController < ApplicationController
  def create
    @user = User.new(user_params)
  end

  private
    def user_params
      params.require(:user).permit(:name, :email_address)
    end
end
"#,
        ),
        // Same list as UsersController — one class, not two.
        (
            "app/controllers/first_runs_controller.rb",
            r#"class FirstRunsController < ApplicationController
  def create
    @user = User.new(user_params)
  end

  private
    def user_params
      params.require(:user).permit(:name, :email_address)
    end
end
"#,
        ),
        (
            "app/controllers/accounts/bots_controller.rb",
            r#"class Accounts::BotsController < ApplicationController
  def update
    @bot.update bot_params
  end

  private
    def bot_params
      params.require(:user).permit(:name, :webhook_url)
    end
end
"#,
        ),
        (
            "app/controllers/users/profiles_controller.rb",
            r#"class Users::ProfilesController < ApplicationController
  def update
    @user.update user_params
  end

  private
    def user_params
      params.require(:user).permit(:name, :email_address, :bio)
    end
end
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

fn fields_of(app: &roundhouse::App, class_name: &str) -> Vec<String> {
    let lcs = lower_controllers_to_library_classes(&app.controllers, Vec::new());
    let lc = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == class_name)
        .unwrap_or_else(|| panic!("{class_name} synthesized"));
    match &lc.origin {
        Some(LibraryClassOrigin::ResourceParams { fields, .. }) => {
            fields.iter().map(|f| f.as_str().to_string()).collect()
        }
        other => panic!("{class_name} is not a ResourceParams class: {other:?}"),
    }
}

#[test]
fn each_distinct_permit_list_gets_its_own_class() {
    let app = campfire_like_app();
    let specs = collect_specs(&app.controllers);
    let user = Symbol::from("user");

    let names: Vec<&str> = specs
        .for_resource(&user)
        .map(|s| s.class_id.0.as_str())
        .collect();
    // Three lists, not four: `UsersController` and `FirstRunsController`
    // permit identically, so they share one class. Order is the order
    // controllers are walked in.
    assert_eq!(
        names,
        vec!["AccountsBotsUserParams", "UserParams", "UsersProfilesUserParams"],
        "one class per distinct (resource, fields) pair"
    );

    // The signup list — the one the collapse used to drop.
    assert_eq!(fields_of(&app, "UserParams"), vec!["name", "email_address"]);
    assert_eq!(
        fields_of(&app, "AccountsBotsUserParams"),
        vec!["name", "webhook_url"],
        "the bots list must NOT gain email_address: that is the mass-assignment boundary"
    );
    assert_eq!(
        fields_of(&app, "UsersProfilesUserParams"),
        vec!["name", "email_address", "bio"]
    );
}

#[test]
fn the_controller_named_for_the_resource_keeps_the_unqualified_name() {
    let app = campfire_like_app();
    let specs = collect_specs(&app.controllers);
    let user = Symbol::from("user");

    let canonical = specs.canonical(&user).expect("a canonical :user spec");
    assert_eq!(canonical.class_id.0.as_str(), "UserParams");
    // `Accounts::BotsController` sorts first, so a first-declarer rule
    // would have handed `UserParams` — and `User.from_params` — to the
    // bot-shaped list.
    assert_eq!(
        canonical.fields.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
        vec!["name", "email_address"]
    );
}

#[test]
fn each_helper_returns_and_builds_its_own_params_class() {
    let app = campfire_like_app();
    let lcs = lower_controllers_to_library_classes(&app.controllers, Vec::new());
    let body_of = |controller: &str, method: &str| -> String {
        let lc = lcs
            .iter()
            .find(|lc| lc.name.0.as_str() == controller)
            .unwrap_or_else(|| panic!("{controller} lowered"));
        let m = lc
            .methods
            .iter()
            .find(|m| m.name.as_str() == method)
            .unwrap_or_else(|| panic!("{controller}#{method}"));
        format!("{:?}", m.body)
    };

    // The helper's OWN permit list names its class — not its method name
    // (`bot_params` permits `:user`), and not the resource's first spec.
    assert!(body_of("UsersController", "user_params").contains("UserParams"));
    assert!(body_of("FirstRunsController", "user_params").contains("UserParams"));
    assert!(body_of("Accounts::BotsController", "bot_params")
        .contains("AccountsBotsUserParams"));
    assert!(body_of("Users::ProfilesController", "user_params")
        .contains("UsersProfilesUserParams"));

    // `User.new(user_params)` in the canonical controller keeps the plain
    // factory; every `update` retargets to the typed variant for its
    // own list.
    assert!(body_of("UsersController", "create").contains("from_params"));
    assert!(body_of("Users::ProfilesController", "update")
        .contains("update_from_users_profiles_user_params"));
    assert!(body_of("Accounts::BotsController", "update")
        .contains("update_from_accounts_bots_user_params"));
}

#[test]
fn the_model_gets_one_typed_surface_per_permit_list() {
    let app = campfire_like_app();
    let specs = collect_specs(&app.controllers);
    let (lcs, _registry) =
        lower_models_with_registry_and_params(&app.models, &app.schema, vec![], &specs);
    let user = lcs.iter().find(|lc| lc.name.0.as_str() == "User").expect("User lowered");
    let assigns = |method: &str| -> Vec<String> {
        let m = user
            .methods
            .iter()
            .find(|m| m.name.as_str() == method)
            .unwrap_or_else(|| panic!("User#{method}"));
        let body = format!("{:?}", m.body);
        ["name", "email_address", "bio", "webhook_url"]
            .iter()
            .filter(|f| body.contains(&format!("{f}=")))
            .map(|f| f.to_string())
            .collect()
    };

    // The canonical list keeps the plain FACTORY name (nothing else
    // competes for it) — including the `email_address` the collapse
    // used to drop.
    assert_eq!(assigns("from_params"), vec!["name", "email_address"]);
    // …but not the plain `update`. That name belongs to Rails'
    // attribute-Hash contract, which is sized to the model's WRITABLE
    // surface, not to any one mass-assignment boundary — so it assigns
    // every column, `bio` and `webhook_url` included.
    assert_eq!(
        assigns("update"),
        vec!["name", "email_address", "bio", "webhook_url"]
    );
    assert_eq!(
        assigns("update!"),
        vec!["name", "email_address", "bio", "webhook_url"]
    );

    // One named typed pair per permit list, canonical included, each
    // sized to its own.
    assert_eq!(assigns("update_from_user_params"), vec!["name", "email_address"]);
    assert_eq!(assigns("update_from_user_params!"), vec!["name", "email_address"]);
    assert_eq!(assigns("from_accounts_bots_user_params"), vec!["name", "webhook_url"]);
    assert_eq!(
        assigns("update_from_accounts_bots_user_params"),
        vec!["name", "webhook_url"]
    );
    assert_eq!(
        assigns("update_from_users_profiles_user_params!"),
        vec!["name", "email_address", "bio"]
    );
}

#[test]
fn a_single_permit_list_per_resource_is_untouched() {
    // The whole corpus is this shape; naming must not churn for it.
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :articles do |t|\n    t.string :title\n    t.text :body\n  end\nend\n",
        ),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        (
            "app/controllers/articles_controller.rb",
            r#"class ArticlesController < ApplicationController
  def create
    @article = Article.new(article_params)
  end

  private
    def article_params
      params.require(:article).permit(:title, :body)
    end
end
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let specs = collect_specs(&app.controllers);
    let article = Symbol::from("article");
    let spec = specs.canonical(&article).expect("canonical :article spec");
    assert_eq!(spec.class_id.0.as_str(), "ArticleParams");

    let (lcs, _registry) =
        lower_models_with_registry_and_params(&app.models, &app.schema, vec![], &specs);
    let model = lcs.iter().find(|lc| lc.name.0.as_str() == "Article").expect("Article lowered");
    let names: Vec<&str> = model.methods.iter().map(|m| m.name.as_str()).collect();
    // The factory keeps the unqualified name — nothing else competes
    // for it, so a single-list app sees no churn there.
    assert!(names.contains(&"from_params"));
    assert!(!names.iter().any(|n| n.starts_with("from_articles_")));
    // `update` / `update!` exist on EVERY model as Rails' attribute-Hash
    // contract, and the typed assignment is named for its permit list
    // whether or not the resource has more than one. Making the name
    // depend on how many lists exist would mean adding a second permit
    // list somewhere in the app silently renames a method the first
    // one's controller calls.
    assert!(names.contains(&"update"));
    assert!(names.contains(&"update!"));
    assert!(names.contains(&"update_from_article_params"));
}
