//! A view sees the ivars its action's `before_action` filters leave
//! behind. Rails runs the callbacks in declaration order, so when two
//! filters write the same ivar the view renders the LAST write:
//!
//! ```ruby
//! before_action :one, :two
//! def one = @x = 1
//! def two = @x = "s"   # the view's @x is "s"
//! ```
//!
//! The view map used to keep the FIRST filter's write, so this view got
//! `Integer` in either declaration order. A filter behind a runtime
//! `if:`/`unless:` may not run, so its write joins the earlier one, even
//! when ingest cannot read the guard. Writes the replay cannot place keep
//! the earlier write, as before.
//!
//! Every case runs with the controllers in both input orders.

use std::path::PathBuf;

use roundhouse::Symbol;
use roundhouse::analyze::{Analyzer, diagnose};
use roundhouse::ide::render_ty;
use roundhouse::ingest::ingest_app_from_tree;

/// `@x = "s"`, the body `two` has unless a case says otherwise.
const S: &str = "    @x = \"s\"";

/// An app with `controllers` (`name`, source) and the view `view` holding `erb`.
fn files(controllers: &[(&str, &str)], view: &str, erb: &str) -> Vec<(String, String)> {
    let mut out = vec![(
        "app/controllers/application_controller.rb".to_string(),
        "class ApplicationController < ActionController::Base\nend\n".to_string(),
    )];
    for (name, src) in controllers {
        out.push((
            format!("app/controllers/{name}_controller.rb"),
            src.to_string(),
        ));
    }
    out.push((format!("app/views/{view}.html.erb"), erb.to_string()));
    out
}

/// The view's `@x` and the `diagnose` messages, with the controllers in
/// either input order.
fn analyze(files: &[(String, String)], view: &str, reverse: bool) -> (String, Vec<String>) {
    let tree = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.clone().into_bytes()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    if reverse {
        app.controllers.reverse();
    }
    Analyzer::new(&app).analyze(&mut app);
    let x = app
        .view_ivar_types
        .get(&Symbol::from(view))
        .and_then(|ivars| ivars.get(&Symbol::from("x")));
    let messages = diagnose(&app).into_iter().map(|d| d.message).collect();
    (
        x.map_or_else(|| "<unbound>".to_string(), render_ty),
        messages,
    )
}

/// The view's `@x`, which both controller input orders must agree on.
fn view_x(files: &[(String, String)], view: &str) -> String {
    let (x, _) = analyze(files, view, false);
    assert_eq!(
        x,
        analyze(files, view, true).0,
        "input order changed {view}'s @x"
    );
    x
}

/// `PostsController` with `filters`, an empty `index`, `one` writing an
/// Integer and `two` with the given body.
fn posts(filters: &str, two: &str) -> String {
    format!(
        "class PostsController < ApplicationController\n{filters}\n  def index; end\n  private\n  def one = (@x = 1)\n  def two\n{two}\n  end\nend\n"
    )
}

fn posts_x(filters: &str, two: &str) -> String {
    let files = files(
        &[("posts", posts(filters, two).as_str())],
        "posts/index",
        "<%= @x %>\n",
    );
    view_x(&files, "posts/index")
}

#[test]
fn the_last_filter_write_reaches_the_view_in_both_declaration_orders() {
    for (filters, expected) in [
        ("  before_action :one, :two", "String"),
        ("  before_action :two, :one", "Integer"),
        ("  before_action :one\n  before_action :two", "String"),
        ("  before_action :two\n  before_action :one", "Integer"),
    ] {
        assert_eq!(posts_x(filters, S), expected, "{filters}");
    }
    // The reported program, and its diagnostic.
    for (declared, expected) in [(":one, :two", "String"), (":two, :one", "Integer")] {
        let p = format!(
            "class PController < ActionController::Base; before_action {declared}; def show; end; private; def one = @x = 1; def two = @x = \"s\"; end\n"
        );
        let files = files(&[("p", p.as_str())], "p/show", "<%= @x.zzz %>\n");
        assert_eq!(view_x(&files, "p/show"), expected);
        let wanted = format!("no known method `zzz` on {expected}");
        assert!(
            analyze(&files, "p/show", false).1.contains(&wanted),
            "{declared}"
        );
    }
}

#[test]
fn a_runtime_guarded_later_filter_joins_the_earlier_write() {
    for guard in [
        "if: :enabled?",
        "unless: :disabled?",
        "if: -> { params[:x] }",
        "unless: -> { params[:x] }",
    ] {
        let filters = format!("  before_action :one\n  before_action :two, {guard}");
        assert_eq!(posts_x(&filters, S), "Integer | String", "{guard}");
    }
    // An unconditional write after the guarded one replaces both.
    let filters = "  before_action :two, if: :enabled?\n  before_action :one";
    assert_eq!(posts_x(filters, S), "Integer");
}

#[test]
fn a_guard_ingest_cannot_read_still_joins() {
    // `if: [:enabled?]` is a Rails guard that ingest does not read; the
    // filter may still be skipped, so its write must not replace.
    let filters = "  before_action :one\n  before_action :two, if: [:enabled?]";
    assert_eq!(posts_x(filters, S), "Integer | String");
    let filters = "  before_action :one\n  before_action(unless: [:enabled?]) { @x = \"s\" }";
    assert_eq!(posts_x(filters, "    nil"), "Integer | String");
}

/// Pins union dispatch, which this change leaves as it is: the guarded
/// join types `@x` as `Integer | String`, and a call that one member
/// answers passes. Rails raises `NoMethodError` on the Integer path.
#[test]
fn union_dispatch_accepts_a_call_one_guarded_member_answers() {
    for guard in [
        "if: :enabled?",
        "if: -> { params[:x] }",
        "unless: :enabled?",
    ] {
        let posts = posts(
            &format!("  before_action :one\n  before_action :two, {guard}"),
            S,
        );
        let files = files(
            &[("posts", posts.as_str())],
            "posts/index",
            "<%= @x.upcase %>\n",
        );
        assert_eq!(view_x(&files, "posts/index"), "Integer | String", "{guard}");
        let messages = analyze(&files, "posts/index", false).1;
        assert!(
            !messages.iter().any(|m| m.contains("upcase")),
            "{guard}: {messages:?}"
        );
    }
}

#[test]
fn an_action_restriction_is_decided_for_the_rendered_action() {
    for (restriction, expected) in [
        ("only: :index", "String"),
        ("only: [:index, :show]", "String"),
        ("only: %i[index show]", "String"),
        ("except: :show", "String"),
        ("except: [:show, :edit]", "String"),
        ("only: :show", "Integer"),
        ("only: %i[show edit]", "Integer"),
        ("except: :index", "Integer"),
        ("except: [:index]", "Integer"),
    ] {
        let filters = format!("  before_action :one\n  before_action :two, {restriction}");
        assert_eq!(posts_x(&filters, S), expected, "{restriction}");
    }
}

#[test]
fn an_inherited_action_view_sees_the_last_write() {
    // `ArchivedPostsController` renders `archived_posts/index` through
    // the parent's `index`, and the parent's filter runs first.
    for (parent, child, expected) in [("one", "two", "String"), ("two", "one", "Integer")] {
        let posts = posts(&format!("  before_action :{parent}"), S);
        let archived = format!(
            "class ArchivedPostsController < PostsController\n  before_action :{child}\nend\n"
        );
        let controllers = [
            ("posts", posts.as_str()),
            ("archived_posts", archived.as_str()),
        ];
        let files = files(&controllers, "archived_posts/index", "<%= @x %>\n");
        assert_eq!(
            view_x(&files, "archived_posts/index"),
            expected,
            "{parent}, {child}"
        );
    }
}

#[test]
fn writes_the_replay_cannot_place_keep_the_earlier_write() {
    // Rails renders the Integer on some path of each: `||=` keeps a
    // truthy value, the guard or the `return` skips the write, the Proc
    // is never called, the around callback writes after `yield`, and the
    // prepended filter runs first.
    for (filters, two) in [
        ("  before_action :one, :two", "    @x ||= \"s\""),
        ("  before_action :one, :two", "    @x = \"s\" if params[:x]"),
        (
            "  before_action :one, :two",
            "    return unless params[:x]\n    @x = \"s\"",
        ),
        (
            "  before_action :one, :two",
            "    saved = proc { @x = \"s\" }",
        ),
        (
            "  before_action :one\n  around_action :two",
            "    yield\n    @x = \"s\"",
        ),
        ("  before_action :one\n  prepend_before_action :two", S),
    ] {
        let x = posts_x(filters, two);
        assert!(
            x.split(" | ").any(|t| t == "Integer"),
            "{filters} / {two}: {x}"
        );
    }
    // A skipped filter does not run.
    let posts = posts("  before_action :one, :two", S);
    let archived =
        "class ArchivedPostsController < PostsController\n  skip_before_action :two\nend\n";
    let controllers = [("posts", posts.as_str()), ("archived_posts", archived)];
    let files = files(&controllers, "archived_posts/index", "<%= @x %>\n");
    assert_eq!(view_x(&files, "archived_posts/index"), "Integer");
}

#[test]
fn the_actions_own_write_still_wins() {
    let posts =
        posts("  before_action :one, :two", S).replace("def index; end", "def index = (@x = 2.5)");
    let files = files(&[("posts", posts.as_str())], "posts/index", "<%= @x %>\n");
    assert_eq!(view_x(&files, "posts/index"), "Float");
}
