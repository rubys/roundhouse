//! View paths (`ingest::view_paths`): `prepend_view_path`,
//! `append_view_path` and `view_paths =`, in an initializer or a class
//! body, decide which file each template name renders, as ActionView 8.1's
//! lookup does. Each template is ingested under the name its renders reach
//! it by, so a test asks which FILE answers a name. What a build cannot
//! evaluate, or cannot render one way for every lookup, is an error.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::diagnostic::{Diagnostic, DiagnosticKind};
use roundhouse::expr::Expr;
use roundhouse::ingest::{ingest_app_from_tree, prism};
use roundhouse::App;

const SCHEMA: &str = "ActiveRecord::Schema.define do\nend\n";
const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base\nend\n";
const LAYOUT: &str = "<html><%= yield %></html>\n";

/// Ingest `files` beside a schema, an `ApplicationController` and a
/// layout; return the app and the view-path errors.
fn ingest(files: &[(&str, &str)]) -> (App, Vec<Diagnostic>) {
    let mut all: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/controllers/application_controller.rb", APPLICATION_CONTROLLER),
        ("app/views/layouts/application.html.erb", LAYOUT),
    ];
    all.extend_from_slice(files);
    let tree: HashMap<PathBuf, Vec<u8>> =
        all.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let (app, diags) = prism::scope(|| ingest_app_from_tree(tree));
    let errors = diags
        .into_iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "view_paths"))
        .collect();
    (app.expect("ingest"), errors)
}

/// The file a template name renders, below `app/views/`, or `None`.
fn origin(app: &App, name: &str, format: &str) -> Option<String> {
    fn file_of(e: &Expr) -> u32 {
        if e.span.file.0 != 0 {
            return e.span.file.0;
        }
        let mut found = 0;
        e.node.for_each_child(&mut |c| {
            if found == 0 {
                found = file_of(c);
            }
        });
        found
    }
    let view = app.views.iter().find(|v| v.name.as_str() == name && v.format.as_str() == format)?;
    let path = &app.sources[file_of(&view.body) as usize - 1].path;
    Some(path[path.find("app/views/").expect("a template under app/views") + "app/views/".len()..].to_string())
}

fn names(app: &App) -> Vec<String> {
    app.views.iter().map(|v| format!("{}.{}", v.name.as_str(), v.format.as_str())).collect()
}

fn controller(name: &str, parent: &str, body: &str) -> String {
    format!("class {name} < {parent}\n{body}\n  def index\n    @title = \"{name}\"\n  end\nend\n")
}

#[test]
fn an_initializer_prepend_makes_the_prepended_template_the_one_each_name_renders() {
    let pages = controller("PagesController", "ApplicationController", "");
    let (app, errors) = ingest(&[
        ("config/initializers/view_paths.rb", "ActionController::Base.prepend_view_path(\"app/views/themes/next\")\n"),
        ("app/controllers/pages_controller.rb", &pages),
        ("app/views/pages/index.html.erb", "old <%= @title %>\n"),
        ("app/views/themes/next/pages/index.html.erb", "new <%= @title %>\n"),
        ("app/views/themes/next/pages/_row.html.erb", "row\n"),
        ("app/views/themes/next/layouts/application.html.erb", LAYOUT),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "pages/index", "html").as_deref(), Some("themes/next/pages/index.html.erb"));
    assert_eq!(origin(&app, "pages/_row", "html").as_deref(), Some("themes/next/pages/_row.html.erb"));
    assert_eq!(origin(&app, "layouts/application", "html").as_deref(), Some("themes/next/layouts/application.html.erb"));
    // Nothing renders them by their `app/views` names, so each file is one template.
    assert!(!names(&app).iter().any(|n| n.starts_with("themes/next/")), "{:?}", names(&app));
    assert_eq!(app.views.len(), 3, "{:?}", names(&app));
}

#[test]
fn initializer_files_run_before_reloader_then_config_to_prepare_blocks_and_on_load_waits_for_its_class() {
    // Rails 8.1 boots these to `[themes/next, themes/edge, themes/beta,
    // app/views]`: the files run in order, but `config.to_prepare` reaches
    // the reloader only after every file, and the hook runs when
    // `ActionController::Base` loads.
    let pages = "class PagesController < ApplicationController\n  def index\n  end\n\n  def show\n  end\n\n  def edit\n  end\nend\n";
    let (app, errors) = ingest(&[
        (
            "config/initializers/a_view_paths.rb",
            "Rails.application.config.to_prepare do\n  ActionController::Base.prepend_view_path Rails.root.join(\"app\", \"views\", \"themes\", \"next\")\nend\n",
        ),
        (
            "config/initializers/b_view_paths.rb",
            "ActiveSupport.on_load(:action_controller) do\n  prepend_view_path \"#{Rails.root}/app/views/themes/beta\"\nend\n",
        ),
        (
            "config/initializers/c_view_paths.rb",
            "ActiveSupport::Reloader.to_prepare do\n  ActionController::Base.prepend_view_path \"app/views/themes/edge\"\nend\n",
        ),
        ("app/controllers/pages_controller.rb", pages),
        ("app/views/themes/next/pages/index.html.erb", "next\n"),
        ("app/views/themes/beta/pages/index.html.erb", "beta\n"),
        ("app/views/themes/beta/pages/show.html.erb", "beta\n"),
        ("app/views/themes/beta/pages/edit.html.erb", "beta\n"),
        ("app/views/themes/edge/pages/index.html.erb", "edge\n"),
        ("app/views/themes/edge/pages/show.html.erb", "edge\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "pages/index", "html").as_deref(), Some("themes/next/pages/index.html.erb"));
    assert_eq!(origin(&app, "pages/show", "html").as_deref(), Some("themes/edge/pages/show.html.erb"));
    assert_eq!(origin(&app, "pages/edit", "html").as_deref(), Some("themes/beta/pages/edit.html.erb"));
}

#[test]
fn rails_configuration_to_prepare_runs_with_the_config_to_prepare_blocks() {
    // `Rails.configuration` is `Rails.application.config`, so this boots
    // to the same `[themes/next, themes/edge, themes/beta, app/views]`.
    let pages = "class PagesController < ApplicationController\n  def index\n  end\n\n  def show\n  end\n\n  def edit\n  end\nend\n";
    let (app, errors) = ingest(&[
        (
            "config/initializers/a_view_paths.rb",
            "Rails.configuration.to_prepare do\n  ActionController::Base.prepend_view_path Rails.root.join(\"app\", \"views\", \"themes\", \"next\")\nend\n",
        ),
        (
            "config/initializers/b_view_paths.rb",
            "ActiveSupport.on_load(:action_controller) do\n  prepend_view_path \"#{Rails.root}/app/views/themes/beta\"\nend\n",
        ),
        (
            "config/initializers/c_view_paths.rb",
            "ActiveSupport::Reloader.to_prepare do\n  ActionController::Base.prepend_view_path \"app/views/themes/edge\"\nend\n",
        ),
        ("app/controllers/pages_controller.rb", pages),
        ("app/views/themes/next/pages/index.html.erb", "next\n"),
        ("app/views/themes/beta/pages/index.html.erb", "beta\n"),
        ("app/views/themes/beta/pages/show.html.erb", "beta\n"),
        ("app/views/themes/beta/pages/edit.html.erb", "beta\n"),
        ("app/views/themes/edge/pages/index.html.erb", "edge\n"),
        ("app/views/themes/edge/pages/show.html.erb", "edge\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "pages/index", "html").as_deref(), Some("themes/next/pages/index.html.erb"));
    assert_eq!(origin(&app, "pages/show", "html").as_deref(), Some("themes/edge/pages/show.html.erb"));
    assert_eq!(origin(&app, "pages/edit", "html").as_deref(), Some("themes/beta/pages/edit.html.erb"));
}

#[test]
fn class_body_prepend_append_and_assignment_change_only_their_own_class() {
    let first = controller("FirstController", "ApplicationController", "  prepend_view_path(\"app/views/alt\") { nil }\n");
    let last = controller("LastController", "ApplicationController", "  append_view_path Rails.root.join(\"app/views/alt\")\n");
    let replaced = controller(
        "ReplacedController",
        "ApplicationController",
        "  self.view_paths = [\"app/views/alt\", \"app/views\"]\n",
    );
    let empty = controller("EmptyController", "ApplicationController", "  self.view_paths = []\n");
    let control = controller("ControlController", "ApplicationController", "");
    let (app, errors) = ingest(&[
        ("app/controllers/first_controller.rb", &first),
        ("app/controllers/last_controller.rb", &last),
        ("app/controllers/replaced_controller.rb", &replaced),
        ("app/controllers/empty_controller.rb", &empty),
        ("app/controllers/control_controller.rb", &control),
        ("app/views/first/index.html.erb", "base\n"),
        ("app/views/alt/first/index.html.erb", "alt\n"),
        ("app/views/last/index.html.erb", "base\n"),
        ("app/views/alt/last/index.html.erb", "alt\n"),
        ("app/views/replaced/index.html.erb", "base\n"),
        ("app/views/alt/replaced/index.html.erb", "alt\n"),
        ("app/views/empty/index.html.erb", "base\n"),
        ("app/views/control/index.html.erb", "base\n"),
        ("app/views/alt/control/index.html.erb", "alt\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "first/index", "html").as_deref(), Some("alt/first/index.html.erb"));
    assert_eq!(origin(&app, "last/index", "html").as_deref(), Some("last/index.html.erb"));
    assert_eq!(origin(&app, "replaced/index", "html").as_deref(), Some("alt/replaced/index.html.erb"));
    assert_eq!(origin(&app, "control/index", "html").as_deref(), Some("control/index.html.erb"));
    // `view_paths = []` leaves the class no template at all.
    assert_eq!(origin(&app, "empty/index", "html"), None);
}

#[test]
fn a_class_without_its_own_list_reads_its_parents_and_one_with_its_own_keeps_it() {
    let parent = controller("ParentController", "ApplicationController", "  prepend_view_path \"app/views/alt\"\n");
    let child = controller("ChildController", "ParentController", "");
    let own = controller("OwnController", "ApplicationController", "");
    let other = controller("OtherController", "ApplicationController", "");
    let (app, errors) = ingest(&[
        // `OwnController` takes its own list before ApplicationController
        // changes its, and so does not see the change; `OtherController`
        // reads it. Rails 8.1 boots the same tree to these answers.
        (
            "config/initializers/view_paths.rb",
            "Rails.application.config.to_prepare do\n  OwnController.append_view_path \"app/views/extra\"\n  ApplicationController.prepend_view_path \"app/views/late\"\nend\n",
        ),
        ("app/controllers/parent_controller.rb", &parent),
        ("app/controllers/child_controller.rb", &child),
        ("app/controllers/own_controller.rb", &own),
        ("app/controllers/other_controller.rb", &other),
        ("app/views/child/index.html.erb", "base\n"),
        ("app/views/alt/child/index.html.erb", "alt\n"),
        ("app/views/own/index.html.erb", "base\n"),
        ("app/views/late/own/index.html.erb", "late\n"),
        ("app/views/other/index.html.erb", "base\n"),
        ("app/views/late/other/index.html.erb", "late\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "child/index", "html").as_deref(), Some("alt/child/index.html.erb"));
    assert_eq!(origin(&app, "own/index", "html").as_deref(), Some("own/index.html.erb"));
    assert_eq!(origin(&app, "other/index", "html").as_deref(), Some("late/other/index.html.erb"));
}

#[test]
fn sibling_controllers_with_opposite_precedence_each_render_their_own_answer() {
    let front = controller("FrontController", "ApplicationController", "  prepend_view_path \"app/views/theme\"\n");
    let back = controller("BackController", "ApplicationController", "  append_view_path \"app/views/theme\"\n");
    let (app, errors) = ingest(&[
        ("app/controllers/front_controller.rb", &front),
        ("app/controllers/back_controller.rb", &back),
        ("app/views/front/index.html.erb", "base <%= render \"form\" %>\n"),
        ("app/views/theme/front/index.html.erb", "theme <%= render \"form\" %>\n"),
        ("app/views/front/_form.html.erb", "base form\n"),
        ("app/views/theme/front/_form.html.erb", "theme form\n"),
        ("app/views/back/index.html.erb", "base\n"),
        ("app/views/theme/back/index.html.erb", "theme\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "front/index", "html").as_deref(), Some("theme/front/index.html.erb"));
    assert_eq!(origin(&app, "front/_form", "html").as_deref(), Some("theme/front/_form.html.erb"));
    assert_eq!(origin(&app, "back/index", "html").as_deref(), Some("back/index.html.erb"));
}

#[test]
fn a_childs_prefix_in_a_later_path_beats_its_parents_prefix_in_the_first() {
    // Rails searches every view path for the child's prefix before the
    // parent's: `child/index` in `app/views`, not `parent/index` in `alt`.
    let parent = controller("ParentController", "ApplicationController", "  prepend_view_path \"app/views/alt\"\n");
    let child = controller("ChildController", "ParentController", "");
    let (app, errors) = ingest(&[
        ("app/controllers/parent_controller.rb", &parent),
        ("app/controllers/child_controller.rb", &child),
        ("app/views/alt/parent/index.html.erb", "alt parent\n"),
        ("app/views/child/index.html.erb", "base child\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "child/index", "html").as_deref(), Some("child/index.html.erb"));
    assert_eq!(origin(&app, "parent/index", "html").as_deref(), Some("alt/parent/index.html.erb"));
}

#[test]
fn each_format_of_a_name_is_looked_up_on_its_own() {
    let streams = controller("StreamsController", "ApplicationController", "  prepend_view_path \"app/views/alt\"\n");
    let (app, errors) = ingest(&[
        ("app/controllers/streams_controller.rb", &streams),
        ("app/views/streams/index.html.erb", "base html\n"),
        ("app/views/alt/streams/index.turbo_stream.erb", "<turbo-stream action=\"append\" target=\"x\"></turbo-stream>\n"),
        ("app/views/streams/index.turbo_stream.erb", "<turbo-stream action=\"remove\" target=\"x\"></turbo-stream>\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "streams/index", "html").as_deref(), Some("streams/index.html.erb"));
    assert_eq!(origin(&app, "streams/index", "turbo_stream").as_deref(), Some("alt/streams/index.turbo_stream.erb"));
}

#[test]
fn a_template_under_a_nested_path_keeps_its_outer_name_only_when_rendered_by_it() {
    let pages = "class PagesController < ApplicationController\n  def index\n    render partial: \"themes/next/shared/outer\"\n  end\nend\n";
    let card = "class Card\n  def html\n    ApplicationController.render(partial: \"themes/next/shared/card\")\n  end\nend\n";
    let (app, errors) = ingest(&[
        ("config/initializers/view_paths.rb", "ActionController::Base.prepend_view_path \"app/views/themes/next\"\n"),
        ("app/controllers/pages_controller.rb", pages),
        ("app/models/card.rb", card),
        ("app/views/themes/next/shared/_card.html.erb", "card\n"),
        ("app/views/themes/next/shared/_outer.html.erb", "outer <%= render \"deep\" %>\n"),
        ("app/views/themes/next/shared/_deep.html.erb", "deep\n"),
        ("app/views/themes/next/shared/_inner.html.erb", "inner\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "themes/next/shared/_outer", "html").as_deref(), Some("themes/next/shared/_outer.html.erb"));
    // What the outer name renders by a relative name keeps its outer name too.
    assert_eq!(origin(&app, "themes/next/shared/_deep", "html").as_deref(), Some("themes/next/shared/_deep.html.erb"));
    assert_eq!(origin(&app, "shared/_deep", "html").as_deref(), Some("themes/next/shared/_deep.html.erb"));
    assert_eq!(origin(&app, "themes/next/shared/_card", "html").as_deref(), Some("themes/next/shared/_card.html.erb"));
    assert_eq!(origin(&app, "shared/_outer", "html").as_deref(), Some("themes/next/shared/_outer.html.erb"));
    assert_eq!(origin(&app, "shared/_inner", "html").as_deref(), Some("themes/next/shared/_inner.html.erb"));
    assert_eq!(origin(&app, "themes/next/shared/_inner", "html"), None);
}

#[test]
fn a_mailers_view_paths_are_its_own_and_its_layout_is_found_through_them() {
    let (app, errors) = ingest(&[
        (
            "app/mailers/application_mailer.rb",
            "class ApplicationMailer < ActionMailer::Base\n  prepend_view_path \"app/views/mailers\"\n  layout \"mailer\"\nend\n",
        ),
        (
            "app/mailers/user_mailer.rb",
            "class UserMailer < ApplicationMailer\n  def welcome\n    mail(to: \"a@example.com\")\n  end\nend\n",
        ),
        ("app/controllers/pages_controller.rb", &controller("PagesController", "ApplicationController", "")),
        ("app/views/pages/index.html.erb", "base\n"),
        ("app/views/mailers/pages/index.html.erb", "mailers\n"),
        ("app/views/mailers/user_mailer/welcome.text.erb", "welcome\n"),
        ("app/views/layouts/mailer.text.erb", "<%= yield %>\n"),
        ("app/views/mailers/layouts/mailer.text.erb", "mailers <%= yield %>\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "user_mailer/welcome", "text").as_deref(), Some("mailers/user_mailer/welcome.text.erb"));
    assert_eq!(origin(&app, "layouts/mailer", "text").as_deref(), Some("mailers/layouts/mailer.text.erb"));
    // A controller's lookup is not the mailers'.
    assert_eq!(origin(&app, "pages/index", "html").as_deref(), Some("pages/index.html.erb"));
}

#[test]
fn two_mailers_whose_paths_find_different_layouts_are_an_error() {
    let mailer = |name: &str, dir: &str| {
        format!("class {name} < ApplicationMailer\n  prepend_view_path \"app/views/{dir}\"\n\n  def note\n    mail(to: \"a@example.com\")\n  end\nend\n")
    };
    let (_, errors) = ingest(&[
        ("app/mailers/application_mailer.rb", "class ApplicationMailer < ActionMailer::Base\n  layout \"mailer\"\nend\n"),
        ("app/mailers/first_mailer.rb", &mailer("FirstMailer", "first")),
        ("app/mailers/second_mailer.rb", &mailer("SecondMailer", "second")),
        ("app/views/first_mailer/note.text.erb", "first\n"),
        ("app/views/second_mailer/note.text.erb", "second\n"),
        ("app/views/first/layouts/mailer.text.erb", "first <%= yield %>\n"),
        ("app/views/second/layouts/mailer.text.erb", "second <%= yield %>\n"),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 1, "{messages:#?}");
    assert!(messages[0].contains("renders `layouts/mailer` (text)"), "{messages:#?}");
}

#[test]
fn what_the_build_cannot_evaluate_is_an_error_not_a_dropped_statement() {
    let (_, errors) = ingest(&[
        (
            "config/initializers/view_paths.rb",
            "ActionController::Base.prepend_view_path(ThemeResolver.new)\nSomeGem::Controller.append_view_path \"app/views/gem\"\n",
        ),
        (
            "app/controllers/themed_controller.rb",
            "class ThemedController < ApplicationController\n  append_view_path theme_dir\n  before_action { prepend_view_path \"app/views/#{params[:theme]}\" }\n\n  def index\n    prepend_view_path \"app/views/alt\"\n  end\nend\n",
        ),
        (
            "config/initializers/mixed.rb",
            "ActiveSupport.on_load(:action_controller) do\n  prepend_view_path \"app/views/alt\"\n  helper :all\nend\n",
        ),
        (
            "app/mailers/application_mailer.rb",
            "class ApplicationMailer < ActionMailer::Base\n  self.view_paths = \"/srv/views\"\nend\n",
        ),
        (
            "app/mailers/sibling_mailer.rb",
            "class SiblingMailer < ApplicationMailer\n  prepend_view_path \"#{Rails.root}_themes/views\"\nend\n",
        ),
        (
            "app/mailers/theme_mailer.rb",
            "class ThemeMailer < ApplicationMailer\n  prepend_view_path Rails.root.join(\"app/views\", \"/srv/themes\")\nend\n",
        ),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    for needle in [
        "a view path that is not a string literal",
        "`SomeGem::Controller`, which is neither a framework base",
        "a class-body block changes the view paths of one `ThemedController` request",
        "`ThemedController#index` changes the view paths of one `ThemedController` request",
        "the view path `/srv/views` is outside the app",
        "an `on_load` block that changes view paths among other statements is not read",
        "the view path `/srv/themes` is outside the app",
    ] {
        assert!(messages.iter().any(|m| m.contains(needle)), "{needle}: {messages:#?}");
    }
    // The unreadable paths: the resolver, the computed `theme_dir` and a
    // sibling of the app's root.
    assert_eq!(messages.iter().filter(|m| m.contains("not a string literal")).count(), 3, "{messages:#?}");
    assert_eq!(errors.len(), 9, "{messages:#?}");
}

#[test]
fn a_name_two_lookups_find_differently_is_an_error_where_it_is_rendered() {
    let themed = "class ThemedController < ApplicationController\n  prepend_view_path \"app/views/theme\"\n\n  def index\n  end\nend\n";
    let plain = controller("PlainController", "ApplicationController", "");
    let (app, errors) = ingest(&[
        ("app/controllers/themed_controller.rb", themed),
        ("app/controllers/plain_controller.rb", &plain),
        ("app/views/theme/themed/index.html.erb", "<%= render \"shared/nav\" %>\n"),
        ("app/views/plain/index.html.erb", "<%= render \"shared/nav\" %>\n"),
        ("app/views/shared/_nav.html.erb", "base nav\n"),
        ("app/views/theme/shared/_nav.html.erb", "theme nav\n"),
        ("app/views/theme/layouts/application.html.erb", LAYOUT),
    ]);
    // `ApplicationController`'s lookup keeps the name; each render that
    // would find the other file says so.
    assert_eq!(origin(&app, "shared/_nav", "html").as_deref(), Some("shared/_nav.html.erb"));
    assert_eq!(origin(&app, "layouts/application", "html").as_deref(), Some("layouts/application.html.erb"));
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 2, "{messages:#?}");
    assert!(
        messages.iter().any(|m| m.contains("template `themed/index` renders `shared/_nav` (html)")
            && m.contains("`app/views/theme/shared/_nav.html.erb`")
            && m.contains("`app/views/shared/_nav.html.erb`")),
        "{messages:#?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("`ThemedController` renders `layouts/application` (html)")),
        "{messages:#?}"
    );
}

#[test]
fn a_template_another_controller_renders_is_judged_under_that_controllers_lookup_too() {
    // `front/index` is the same file for both, but its `render "row"` finds
    // `theme/front/_row` for Front and `front/_row` for Plain.
    let front = controller("FrontController", "ApplicationController", "  prepend_view_path \"app/views/theme\"\n");
    let plain = "class PlainController < ApplicationController\n  def index\n    render \"front/index\"\n  end\nend\n";
    let (_, errors) = ingest(&[
        ("app/controllers/front_controller.rb", &front),
        ("app/controllers/plain_controller.rb", plain),
        ("app/views/front/index.html.erb", "<%= render \"row\" %>\n"),
        ("app/views/front/_row.html.erb", "base row\n"),
        ("app/views/theme/front/_row.html.erb", "theme row\n"),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 1, "{messages:#?}");
    assert!(
        messages[0].contains("template `front/index` renders `front/_row` (html)")
            && messages[0].contains("find at `app/views/front/_row.html.erb`")
            && messages[0].contains("renders from `app/views/theme/front/_row.html.erb`"),
        "{messages:#?}"
    );
}

#[test]
fn a_controller_without_a_layout_is_not_judged_by_one() {
    let bare = controller(
        "BareController",
        "ApplicationController",
        "  layout false\n  prepend_view_path \"app/views/theme\"\n",
    );
    let fragment = controller(
        "FragmentController",
        "ApplicationController",
        "  layout false, only: [:index, :show]\n  prepend_view_path \"app/views/theme\"\n\n  def show\n    render \"shared/page\"\n  end\n",
    );
    let partial = "class PartialController < ApplicationController\n  prepend_view_path \"app/views/theme\"\n\n  def index\n    render :index, layout: false\n  end\n\n  def feed\n    render template: \"shared/feed\", formats: :html, layout: false\n  end\nend\n";
    let plain = "class PlainController < ApplicationController\n  def index\n    render template: \"shared/feed\"\n  end\nend\n";
    let (app, errors) = ingest(&[
        ("app/controllers/bare_controller.rb", &bare),
        ("app/controllers/fragment_controller.rb", &fragment),
        ("app/controllers/partial_controller.rb", partial),
        ("app/controllers/plain_controller.rb", plain),
        ("app/views/theme/partial/index.html.erb", "partial\n"),
        ("app/views/shared/feed.html.erb", "feed\n"),
        ("app/views/shared/feed.json.erb", "{}\n"),
        ("app/views/theme/shared/feed.json.erb", "[]\n"),
        ("app/views/theme/bare/index.html.erb", "bare\n"),
        ("app/views/theme/fragment/index.html.erb", "fragment\n"),
        ("app/views/shared/page.html.erb", "page\n"),
        ("app/views/theme/layouts/application.html.erb", LAYOUT),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "bare/index", "html").as_deref(), Some("theme/bare/index.html.erb"));
}

#[test]
fn an_on_load_hook_with_options_runs_like_one_without() {
    let pages = controller("PagesController", "ApplicationController", "");
    let (app, errors) = ingest(&[
        (
            "config/initializers/view_paths.rb",
            "ActiveSupport.on_load(:action_controller, run_once: true) do\n  prepend_view_path \"app/views/theme\"\nend\n",
        ),
        ("app/controllers/pages_controller.rb", &pages),
        ("app/views/pages/index.html.erb", "base\n"),
        ("app/views/theme/pages/index.html.erb", "theme\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "pages/index", "html").as_deref(), Some("theme/pages/index.html.erb"));
}

#[test]
fn a_run_once_hook_on_both_controller_bases_the_app_uses_is_an_error() {
    let api = "class ApiController < ActionController::API\n  def index\n  end\nend\n";
    let (_, errors) = ingest(&[
        (
            "config/initializers/view_paths.rb",
            "ActiveSupport.on_load(:action_controller, run_once: true) do\n  prepend_view_path \"app/views/theme\"\nend\n",
        ),
        ("app/controllers/pages_controller.rb", &controller("PagesController", "ApplicationController", "")),
        ("app/controllers/api_controller.rb", api),
        ("app/views/pages/index.html.erb", "base\n"),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 1, "{messages:#?}");
    assert!(messages[0].contains("a `run_once` hook on controller bases the app uses both of"), "{messages:#?}");
}

#[test]
fn a_run_once_hook_runs_its_whole_body_on_the_one_base_the_app_uses() {
    let pages = controller("PagesController", "ApplicationController", "");
    let (app, errors) = ingest(&[
        (
            "config/initializers/view_paths.rb",
            "ActiveSupport.on_load(:action_controller, run_once: true) do\n  prepend_view_path \"app/views/theme\"\n  append_view_path \"app/views/extra\"\nend\n",
        ),
        ("app/controllers/pages_controller.rb", &pages),
        ("app/views/theme/pages/index.html.erb", "theme\n"),
        ("app/views/extra/pages/show.html.erb", "extra\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "pages/index", "html").as_deref(), Some("theme/pages/index.html.erb"));
    assert_eq!(origin(&app, "pages/show", "html").as_deref(), Some("extra/pages/show.html.erb"));
}

#[test]
fn a_to_prepare_statement_that_names_a_class_loads_it_before_later_changes() {
    // Rails loads ApplicationController at the `include`; its body's append
    // takes its own list then, which the later prepend on the base misses.
    let application = "class ApplicationController < ActionController::Base\n  append_view_path \"app/views/extra\"\nend\n";
    let pages = controller("PagesController", "ApplicationController", "");
    let (app, errors) = ingest(&[
        (
            "config/initializers/view_paths.rb",
            "Rails.application.config.to_prepare do\n  ApplicationController.include Guard\n  ActionController::Base.prepend_view_path \"app/views/theme\"\nend\n",
        ),
        // A method body names its constant only when called.
        (
            "config/initializers/a_helper.rb",
            "Rails.application.config.to_prepare do\n  def themed_class\n    ThemedController\n  end\nend\n",
        ),
        (
            "app/controllers/themed_controller.rb",
            "class ThemedController < ActionController::Base\n  append_view_path \"app/views/more\"\n\n  def index\n  end\nend\n",
        ),
        ("app/views/theme/themed/index.html.erb", "theme\n"),
        ("app/controllers/application_controller.rb", application),
        ("app/controllers/concerns/guard.rb", "module Guard\nend\n"),
        ("app/controllers/pages_controller.rb", &pages),
        ("app/views/pages/index.html.erb", "base\n"),
        ("app/views/theme/pages/index.html.erb", "theme\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "pages/index", "html").as_deref(), Some("pages/index.html.erb"));
    assert_eq!(origin(&app, "themed/index", "html").as_deref(), Some("theme/themed/index.html.erb"));
}

#[test]
fn an_inherited_action_renders_through_the_subclass_lookup() {
    let parent = "class ParentController < ApplicationController\n  def index\n    render \"shared/page\"\n  end\nend\n";
    let child = "class ChildController < ParentController\n  prepend_view_path \"app/views/theme\"\nend\n";
    let (_, errors) = ingest(&[
        ("app/controllers/parent_controller.rb", parent),
        ("app/controllers/child_controller.rb", child),
        ("app/views/shared/page.html.erb", "base\n"),
        ("app/views/theme/shared/page.html.erb", "theme\n"),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 1, "{messages:#?}");
    assert!(messages[0].contains("`ChildController` renders `shared/page` (html)"), "{messages:#?}");
}

#[test]
fn a_class_level_render_and_a_shared_templates_layout_go_through_the_controllers_lookup() {
    // Only ThemedController renders `shared/_nav`, so its file is kept;
    // its layout wraps `shared/page`, and that name it shares.
    let themed = "class ThemedController < ApplicationController\n  prepend_view_path \"app/views/theme\"\n\n  def index\n    render \"shared/page\"\n  end\nend\n";
    let (app, errors) = ingest(&[
        ("app/controllers/themed_controller.rb", themed),
        (
            "app/models/card.rb",
            "class Card\n  def html\n    ThemedController.render(partial: \"shared/nav\")\n  end\nend\n",
        ),
        ("app/views/shared/page.html.erb", "page\n"),
        ("app/views/shared/_nav.html.erb", "base nav\n"),
        ("app/views/theme/shared/_nav.html.erb", "theme nav\n"),
        ("app/views/theme/layouts/application.html.erb", LAYOUT),
    ]);
    assert_eq!(origin(&app, "shared/_nav", "html").as_deref(), Some("theme/shared/_nav.html.erb"));
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 1, "{messages:#?}");
    assert!(messages[0].contains("`ThemedController` renders `layouts/application` (html)"), "{messages:#?}");
}

#[test]
fn an_inherited_action_without_its_own_template_is_judged_under_the_subclass_lookup() {
    // Rails renders the parent's `index` through the child's paths:
    // `theme/parent/index`, where the parent's own lookup keeps another.
    let parent = controller("ParentController", "ApplicationController", "");
    let child = "class ChildController < ParentController\n  prepend_view_path \"app/views/theme\"\nend\n";
    let (_, errors) = ingest(&[
        ("app/controllers/parent_controller.rb", &parent),
        ("app/controllers/child_controller.rb", child),
        ("app/views/parent/index.html.erb", "base\n"),
        ("app/views/theme/parent/index.html.erb", "theme\n"),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 1, "{messages:#?}");
    assert!(messages[0].contains("`ChildController` renders `parent/index` (html)"), "{messages:#?}");
}

#[test]
fn an_inherited_relative_render_is_judged_and_an_action_that_responds_is_not() {
    // `show` renders `render "show"` through Child's prefixes, finding
    // `theme/parent/show`; `index` answers `head` and renders nothing.
    let parent = "class ParentController < ApplicationController\n  def index\n    head :no_content\n  end\n\n  def show\n    render \"show\"\n  end\nend\n";
    let child = "class ChildController < ParentController\n  prepend_view_path \"app/views/theme\"\nend\n";
    let (_, errors) = ingest(&[
        ("app/controllers/parent_controller.rb", parent),
        ("app/controllers/child_controller.rb", child),
        ("app/views/parent/index.html.erb", "base\n"),
        ("app/views/theme/parent/index.html.erb", "theme\n"),
        ("app/views/parent/show.html.erb", "base\n"),
        ("app/views/theme/parent/show.html.erb", "theme\n"),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 1, "{messages:#?}");
    assert!(messages[0].contains("`ChildController` renders `parent/show` (html)"), "{messages:#?}");
}

#[test]
fn a_concerns_render_goes_through_the_including_controllers_lookup() {
    let themed = "class ThemedController < ApplicationController\n  include Paging\n  prepend_view_path \"app/views/theme\"\nend\n";
    let concern = "module Paging\n  extend ActiveSupport::Concern\n\n  def page\n    render \"shared/page\"\n  end\nend\n";
    let plain = "class PlainController < ApplicationController\n  def index\n    render \"shared/page\"\n  end\nend\n";
    let (_, errors) = ingest(&[
        ("app/controllers/concerns/paging.rb", concern),
        ("app/controllers/themed_controller.rb", themed),
        ("app/controllers/plain_controller.rb", plain),
        ("app/views/shared/page.html.erb", "base\n"),
        ("app/views/theme/shared/page.html.erb", "theme\n"),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert!(messages.iter().any(|m| m.contains("`ThemedController` renders `shared/page` (html)")), "{messages:#?}");
}

#[test]
fn a_superclass_name_two_classes_answer_with_different_view_paths_is_an_error() {
    let base = "class BaseController < ApplicationController\nend\n";
    let admin_base = "module Admin\n  class BaseController < ApplicationController\n    prepend_view_path \"app/views/admin_theme\"\n  end\nend\n";
    let pages = "class Admin::PagesController < BaseController\n  def index\n  end\nend\n";
    let rooted = "class Admin::RootedController < ::BaseController\n  def index\n  end\nend\n";
    let (_, errors) = ingest(&[
        ("app/controllers/base_controller.rb", base),
        ("app/controllers/admin/base_controller.rb", admin_base),
        ("app/controllers/admin/pages_controller.rb", pages),
        ("app/controllers/admin/rooted_controller.rb", rooted),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 1, "{messages:#?}");
    assert!(messages[0].contains("`Admin::PagesController`'s superclass `BaseController` may be `Admin::BaseController` or `::BaseController`"), "{messages:#?}");
}

#[test]
fn a_layout_is_judged_only_in_the_formats_its_user_renders() {
    // Both use `layout "shared"`; only the mailer renders text, and its
    // path has the text layout.
    let pages = controller("PagesController", "ApplicationController", "  layout \"shared\"\n");
    let (app, errors) = ingest(&[
        ("app/controllers/pages_controller.rb", &pages),
        ("app/views/pages/index.html.erb", "page\n"),
        ("app/views/layouts/shared.html.erb", LAYOUT),
        (
            "app/mailers/note_mailer.rb",
            "class NoteMailer < ActionMailer::Base\n  layout \"shared\"\n  prepend_view_path \"app/views/mail\"\n\n  def note\n    mail(to: \"a@example.com\")\n  end\nend\n",
        ),
        ("app/views/mail/note_mailer/note.text.erb", "note\n"),
        ("app/views/mail/layouts/shared.text.erb", "<%= yield %>\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(origin(&app, "layouts/shared", "text").as_deref(), Some("mail/layouts/shared.text.erb"));
}

#[test]
fn an_inherited_template_is_wrapped_in_the_subclass_layout() {
    // Child renders `parent/index` (the same file for both) inside
    // `theme/layouts/application`, which the build does not keep.
    let parent = controller("ParentController", "ApplicationController", "");
    let child = "class ChildController < ParentController\n  prepend_view_path \"app/views/theme\"\nend\n";
    let (_, errors) = ingest(&[
        ("app/controllers/parent_controller.rb", &parent),
        ("app/controllers/child_controller.rb", child),
        ("app/views/parent/index.html.erb", "page\n"),
        ("app/views/theme/layouts/application.html.erb", LAYOUT),
    ]);
    let messages: Vec<&str> = errors.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(errors.len(), 1, "{messages:#?}");
    assert!(messages[0].contains("`ChildController` renders `layouts/application` (html)"), "{messages:#?}");
}

#[test]
fn an_app_without_view_paths_reads_app_views_as_before() {
    let (app, errors) = ingest(&[
        ("app/controllers/pages_controller.rb", &controller("PagesController", "ApplicationController", "")),
        ("app/views/pages/index.html.erb", "index\n"),
        ("app/views/themes/next/pages/index.html.erb", "next\n"),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        names(&app),
        ["layouts/application.html", "pages/index.html", "themes/next/pages/index.html"]
    );
}
