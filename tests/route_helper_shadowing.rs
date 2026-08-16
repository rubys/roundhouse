//! A controller method named `<x>_path` / `<x>_url` shadows the route
//! helper of the same name (`controller_to_library::rewrites::
//! rewrite_route_helpers`).
//!
//! Rails injects route helpers by module inclusion, so a `def` anywhere
//! in the controller's ancestry wins over them. The rewrite that gives
//! bare helper calls their `RouteHelpers.` receiver keyed off the name
//! SUFFIX alone, which is a heuristic Ruby does not share: campfire's
//! `redirect_to post_authenticating_url` — a private method on the
//! Authentication concern, spliced into ApplicationController — became
//! `RouteHelpers.post_authenticating_path`, a module with no such
//! function. Mastodon has 120 controller methods with this shape.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_controllers_to_library_classes;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :articles do |t|\n    t.string :title\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    resources :articles\n  root \"articles#index\"\nend\n";

fn emitted(controllers: &[(&str, &str)]) -> String {
    let mut files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
    ];
    files.extend_from_slice(controllers);
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let classes = lower_controllers_to_library_classes(&app.controllers, Vec::new());
    format!("{:?}", classes)
}

#[test]
fn a_controllers_own_method_is_not_a_route_helper() {
    let out = emitted(&[(
        "app/controllers/articles_controller.rb",
        r#"
class ArticlesController < ApplicationController
  def index
    redirect_to landing_url
  end

  private
    def landing_url
      "/somewhere"
    end
end
"#,
    )]);
    assert!(
        !out.contains("landing_path"),
        "a defined method must not be folded onto a RouteHelpers `_path` twin:\n{out}"
    );
}

#[test]
fn an_ancestors_method_shadows_too() {
    // The shape campfire has: the method is defined on
    // ApplicationController (spliced there from a concern), and the
    // call site is three classes away.
    let out = emitted(&[
        (
            "app/controllers/application_controller.rb",
            r#"
class ApplicationController < ActionController::Base
  private
    def post_authenticating_url
      "/after"
    end
end
"#,
        ),
        (
            "app/controllers/articles_controller.rb",
            r#"
class ArticlesController < ApplicationController
  def index
    redirect_to post_authenticating_url
  end
end
"#,
        ),
    ]);
    assert!(
        !out.contains("post_authenticating_path"),
        "an inherited method must shadow the route helper too:\n{out}"
    );
}

#[test]
fn a_real_route_helper_still_gets_its_receiver() {
    let out = emitted(&[(
        "app/controllers/articles_controller.rb",
        r#"
class ArticlesController < ApplicationController
  def index
    redirect_to articles_url
  end
end
"#,
    )]);
    assert!(
        out.contains("RouteHelpers"),
        "a name nothing defines is still a route helper:\n{out}"
    );
}
