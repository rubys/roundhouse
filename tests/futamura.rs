//! `--target futamura`, stage 0: the identity.
//!
//! The target's mandate is partial evaluation — specialize what the
//! analysis resolves, leave the rest as residual Ruby running on the
//! app's real gems. Stage 0 specializes nothing, so the residue is the
//! whole app and the output must be the app as source control holds it.
//! These gates exist before the first specialization so that every
//! later one is measured against the same contract: the app's own
//! suite, run by Rails, passes against the emitted tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-futamura-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// A tree with source the identity must carry (dotfiles, `.keep`
/// placeholders, a binary asset, an executable) and run-time state it
/// must not (logs, databases, caches, the master key).
fn app_tree(root: &Path) {
    write(root, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(root, ".ruby-version", b"ruby-4.0.2\n");
    write(root, ".gitignore", b"/log/*\n/tmp/*\n");
    write(root, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"up\" => \"rails/health#show\", as: :rails_health_check\nend\n");
    write(root, "app/models/application_record.rb", b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n");
    write(root, "app/models/widget.rb", b"class Widget < ApplicationRecord\n  def method_missing(name, *args) = name\nend\n");
    write(root, "app/assets/images/logo.png", PNG);
    write(root, "bin/rails", b"#!/usr/bin/env ruby\n");
    write(root, "log/.keep", b"");
    write(root, "log/test.log", b"noise\n");
    write(root, "tmp/.keep", b"");
    write(root, "tmp/cache/x", b"cached\n");
    write(root, "storage/.keep", b"");
    write(root, "storage/test.sqlite3", b"SQLite format 3\0");
    write(root, "config/master.key", b"secret\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join("bin/rails"), fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn identity_carries_source_and_drops_run_time_state() {
    let src = scratch("src");
    let out = scratch("out");
    app_tree(&src);
    let status = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    // `method_missing` is outside what the strict targets type, and the
    // other targets drop Rails' own health controller; here both are
    // residue Rails runs, so neither is a diagnostic at all.
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(!stderr.contains("error") && !stderr.contains("warning"), "{stderr}");

    for rel in [
        "Gemfile",
        ".ruby-version",
        ".gitignore",
        "config/routes.rb",
        "app/models/widget.rb",
        "app/assets/images/logo.png",
        "bin/rails",
        "log/.keep",
        "tmp/.keep",
        "storage/.keep",
    ] {
        assert_eq!(fs::read(out.join(rel)).ok(), fs::read(src.join(rel)).ok(), "{rel}");
    }
    for rel in ["log/test.log", "tmp/cache/x", "storage/test.sqlite3", "config/master.key"] {
        assert!(!out.join(rel).exists(), "{rel} is run-time state, not source");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(out.join("bin/rails")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "bin/rails must stay executable");
    }
    // The source app ships no README, so the target supplies one.
    assert!(fs::read_to_string(out.join("README.md")).unwrap().contains("bin/rails test"));

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// The behavioral gate: `scripts/futamura-suite` emits `fixtures/real-blog`
/// and runs its own suite with Rails against a copy of the source and
/// against the emitted tree; the two must give the same result. The
/// script is the gate's one definition, also used for apps outside the
/// repo (campfire). Needs the generated fixture and its bundle.
#[test]
#[ignore = "requires fixtures/real-blog and its Rails bundle"]
fn real_blog_suite_gives_the_same_result_on_source_and_emitted_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("fixtures/real-blog");
    assert!(fixture.join("Gemfile.lock").exists(), "generate fixtures/real-blog first");
    let mut cmd = Command::new(root.join("scripts/futamura-suite"));
    cmd.arg(&fixture)
        .env("ROUNDHOUSE_BIN", env!("CARGO_BIN_EXE_roundhouse"))
        .env_remove("BUNDLE_GEMFILE");
    // Whole-route replacement needs Ruby 4.1 started with RUBY_BOX=1.
    // Run this under such a Ruby with RUBY_BOX=1 set and the suite's GET
    // requests are served by the lowered app inside Rails; otherwise the
    // emitted tree runs with whole-route off, as plain Rails.
    if std::env::var("RUBY_BOX").as_deref() != Ok("1") {
        cmd.env("FUTAMURA_WHOLE_ROUTE", "off");
    }
    let run = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    // The fixture's suite is green, so SAME must mean zero failures.
    assert!(stdout.contains(" 0 failures, 0 errors"), "{stdout}");
}

/// Whole-route replacement: a GET action whose code reaches no error
/// under the `ruby` analysis is served by the lowered app; one that does
/// stays with Rails, and the ledger names the error and the path to it.
/// Writes are never considered.
#[test]
fn whole_route_lowers_clean_get_actions_and_leaves_the_rest_to_rails() {
    let src = scratch("wr-src");
    let out = scratch("wr-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "db/schema.rb", br#"ActiveRecord::Schema.define do
  create_table "posts", force: :cascade do |t|
    t.string "title", null: false
  end
end
"#);
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  resources :posts, only: [:index, :show, :create]\nend\n");
    write(&src, "app/models/application_record.rb", b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n");
    write(&src, "app/models/post.rb", b"class Post < ApplicationRecord\n  def shout\n    title.frobnicate\n  end\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/posts_controller.rb", br#"class PostsController < ApplicationController
  def index
    @posts = Post.all
  end

  def show
    @post = Post.find(params[:id])
    @shout = @post.shout
  end

  def create
    @post = Post.create!(title: params[:title])
    redirect_to posts_path
  end
end
"#);
    write(&src, "app/views/layouts/application.html.erb", b"<html><body><%= yield %></body></html>\n");
    write(&src, "app/views/posts/index.html.erb", b"<% @posts.each do |post| %><p><%= post.title %></p><% end %>\n");
    write(&src, "app/views/posts/show.html.erb", b"<h1><%= @post.title %></h1><p><%= @shout %></p>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    let residue = stderr.lines().find(|l| l.contains("rails ") && l.contains("posts#show")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(residue.contains("app/models/post.rb:3") && residue.contains("frobnicate"), "{residue}");
    assert!(residue.contains("PostsController#show -> Post#shout"), "{residue}");

    let init = fs::read_to_string(out.join("config/initializers/futamura_routes.rb")).unwrap();
    assert!(init.contains("    posts#index\n"), "{init}");
    assert!(!init.contains("posts#show") && !init.contains("posts#create"), "{init}");
    assert!(out.join("vendor/futamura/bridge.rb").exists());
    assert!(out.join("vendor/futamura/lowered/main.rb").exists());
    // The lowered app's own tests are not part of the served program.
    assert!(!out.join("vendor/futamura/lowered/test").exists());
    // The app itself is still the app.
    assert_eq!(fs::read(out.join("app/models/post.rb")).unwrap(), fs::read(src.join("app/models/post.rb")).unwrap());
}

/// A receiverless call in a controller (`request`, `params`) dispatches
/// on the controller's own lookup — its ancestors and included modules —
/// never on an unrelated concrete class that happens to define a method
/// of the same name. Two ways the planner used to get this wrong, fixed
/// together:
///
///   * a bare receiverless call not found on the class used to fan out
///     to every app definer, including plain classes (here, `Gadget`);
///   * `request.x` was treated as reaching app code merely because some
///     app class defines `request`, instead of asking whether the
///     *current* class's own lookup resolves it (here it does not, so
///     `request` is the runtime's, and `.path` never reaches
///     `PathExploder#path`).
///
/// Neither `Gadget` nor `PathExploder` is reachable from `widgets#index`,
/// so the route is clean and clears.
#[test]
fn receiverless_calls_resolve_through_the_controllers_own_lookup_not_an_unrelated_class() {
    let src = scratch("recv-src");
    let out = scratch("recv-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def index
    @path = request.path
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @path %>\n");
    // A plain (non-module) class that happens to define `request`: the
    // planner must not treat `WidgetsController`'s bare `request` call as
    // reaching it.
    write(&src, "app/lib/gadget.rb", br#"class Gadget
  def request
    Frobnicator::Spinner.new.spin
  end
end
"#);
    // A plain class defining `path`: only reachable if `request.path` is
    // wrongly treated as app-rooted once `request` itself resolves to
    // nothing on the controller.
    write(&src, "app/lib/path_exploder.rb", br#"class PathExploder
  def path
    Combobulator::Unit.new.run
  end
end
"#);

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");
    assert!(!stderr.contains("Gadget") && !stderr.contains("PathExploder"), "{stderr}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// #957's gap rule only checked the route's own controller file and its
/// conventional view path. A gap in a file the route's reach actually
/// walks into — here the layout every action renders through, via
/// `apply_layout_lowering`'s `render(Views::Layouts.application(...))`
/// wrap — went unblocked: the layout still lowers to a real
/// `Views::Layouts.application` method (the dropped
/// `vite_typescript_tag` call is only a gap, not a parse failure), so
/// the route used to clear even though the lowered app cannot serve
/// what that tag would have rendered. `routes::plan` now also blocks on
/// any gap file the reach walk's `reached_files` contains.
#[test]
fn ingest_gap_in_a_rendered_layout_blocks_the_route() {
    let src = scratch("layout-gap-src");
    let out = scratch("layout-gap-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");
    // No Vite manifest: the tag call stays an unresolved gap on the
    // layout's own file, not a parse failure — the layout still becomes
    // a real `Views::Layouts.application` method the route's reach
    // walks into.
    write(&src, "app/views/layouts/application.html.erb", b"<html><head><%= vite_typescript_tag \"application\" %></head><body><%= yield %></body></html>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    let residue = stderr.lines().find(|l| l.contains("widgets#index")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(residue.contains("ingest gap") && residue.contains("layouts/application.html.erb") && residue.contains("vite_typescript_tag"), "{residue}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// Companion to `ingest_gap_in_a_rendered_layout_blocks_the_route`: on a
/// large real app, a layout's `vite_typescript_tag "sign_up.ts",
/// crossorigin: "anonymous"` was exactly this shape — a manifest entry
/// resolves, but the extra `crossorigin:` keyword made the whole call an
/// ingest gap, so the gap in the rendered layout blocked every route
/// (301 of 314 on that app). With the pass-through modeled, the same
/// layout lowers cleanly and the route clears.
#[test]
fn a_vite_tag_with_a_crossorigin_option_does_not_block_the_route() {
    let src = scratch("layout-crossorigin-src");
    let out = scratch("layout-crossorigin-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");
    write(
        &src,
        "public/vite/.vite/manifest.json",
        br#"{"entrypoints/application.ts": {"file": "assets/application-abc.js", "isEntry": true}}"#,
    );
    write(&src, "app/views/layouts/application.html.erb", b"<html><head><%= vite_typescript_tag \"application\", crossorigin: \"anonymous\" %></head><body><%= yield %></body></html>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");
    assert!(!stderr.contains("vite_typescript_tag"), "{stderr}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// A gap in a parent controller's class body (here an `around_action`
/// block shape the ingester refuses, round-tripped as `Unknown` rather
/// than lowered into the filter chain) must block every subclass route,
/// not only routes that reach the parent's file through a call —
/// #957's rule named only the route's own controller file, so a gap in
/// `ApplicationController` left every subclass route unblocked.
#[test]
fn ingest_gap_in_a_parent_controllers_class_body_blocks_the_route() {
    let src = scratch("parent-gap-src");
    let out = scratch("parent-gap-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    // Three block parameters: a shape `around_block_filter` refuses
    // (only 0, 1, or 2 — controller and continuation — are supported),
    // recorded as a gap on this file and kept as `Unknown` rather than
    // becoming a filter. `unrelated_helper` is never called by
    // `widgets#index`, so nothing about this route's own reach touches
    // this file either — only the ancestor-chain check does.
    write(&src, "app/controllers/application_controller.rb", br#"class ApplicationController < ActionController::Base
  around_action { |controller, block, extra| block.call }

  def unrelated_helper
    "ok"
  end
end
"#);
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    let residue = stderr.lines().find(|l| l.contains("widgets#index")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(residue.contains("ingest gap") && residue.contains("application_controller.rb") && residue.contains("around_action block declares 3 parameters"), "{residue}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// A gap in an `app/helpers/*_helper.rb` module must block every route
/// that renders a view, not only ones whose reach happens to touch
/// that file — Rails mixes every helper module into every view
/// (`helper :all`, the default), and `ingest_library_classes` rejecting
/// the helper outright means it was never a `LibraryClass` `reach`
/// could have added to the route's own `blocking_files` in the first
/// place (CodeRabbit 4281788796: the planner could replace a route
/// while the emit lacks the helper, a NoMethodError at runtime).
#[test]
fn ingest_gap_in_a_helper_module_blocks_every_route_that_renders_a_view() {
    let src = scratch("helper-gap-src");
    let out = scratch("helper-gap-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");
    // Mixing a `cattr_accessor ... default:` with a source `@@` write
    // is a recorded ingest gap (`class-variable initializer with
    // non-nil value`), on `application_helper.rb` — a file nothing in
    // `widgets#index`'s own reach ever calls into.
    write(&src, "app/helpers/application_helper.rb", br#"module ApplicationHelper
  cattr_accessor :locale, default: "en"
  @@locale = "fr"
end
"#);

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    let residue = stderr.lines().find(|l| l.contains("widgets#index")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(residue.contains("ingest gap") && residue.contains("application_helper.rb"), "{residue}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// The extended gap rule only blocks more: a route whose reached files
/// (its own controller, its ancestor chain, and the layout it renders
/// through) carry no gap at all still clears, same as before #957's
/// rule was extended.
#[test]
fn route_with_clean_reached_files_still_clears() {
    let src = scratch("clean-reach-src");
    let out = scratch("clean-reach-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", br#"class ApplicationController < ActionController::Base
  around_action :with_timing

  def with_timing
    yield
  end
end
"#);
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/layouts/application.html.erb", b"<html><body><%= yield %></body></html>\n");
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(!stderr.contains("ingest gap"), "{stderr}");
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// A view template format the ingester does not model (here `.ics.erb`
/// — `erb` is only ingested for `html` and a short allow-list of other
/// text formats) never becomes a `View`, so the action's `render` leaves
/// no trace the planner can walk into: nothing reaches an error, and the
/// route used to clear. The ingest gap this leaves behind has no span,
/// so it is attributed by file: the gap's view template matches
/// `gadget_styles#show`'s conventional path
/// (`app/views/gadget_styles/show.`), which blocks the route even
/// though its (ingested) code is clean.
#[test]
fn ingest_gaps_block_the_routes_whose_view_they_belong_to() {
    let src = scratch("gap-src");
    let out = scratch("gap-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"gadget.ics\", to: \"gadget_styles#show\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/gadget_styles_controller.rb", br#"class GadgetStylesController < ApplicationController
  def show
    @color = "teal"
    render content_type: "text/calendar"
  end
end
"#);
    write(&src, "app/views/gadget_styles/show.ics.erb", b"BEGIN:VCALENDAR\nX-COLOR:<%= @color %>\nEND:VCALENDAR\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    let residue = stderr.lines().find(|l| l.contains("gadget_styles#show")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(residue.contains("ingest gap") && residue.contains("show.ics.erb"), "{residue}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// A library module whose `include` names an external gem module the
/// analyzer cannot resolve has no span to blame (ingest drops the
/// call's span once `LibraryClass.includes` resolves the name), so the
/// refusal used to block every route. Carrying the span (`include_spans`)
/// gives it a real location, and the planner's existing per-file rule
/// then blocks only the routes that reach `GadgetTools`' file —
/// `widgets#index` never does, so it still clears. That's sound only if
/// the box still LOADS with the include unresolved: futamura's own copy
/// of the `ruby` emit guards it (`include X if defined?(X)`); plain
/// `--target ruby` does not, and stays fail-loud.
#[test]
fn include_locations_free_routes_that_never_reach_them_and_the_box_still_loads() {
    let src = scratch("inc-src");
    let out = scratch("inc-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");
    // Never reached by `widgets#index`: a library module that includes
    // an unresolved external gem module, and a class that mixes it in.
    write(&src, "app/lib/gadget_tools.rb", br#"module GadgetTools
  include ExternalGem::Mixin

  def polish
    "shiny"
  end
end
"#);
    write(&src, "app/lib/gadget.rb", b"class Gadget\n  include GadgetTools\nend\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    let unowned = stderr
        .lines()
        .find(|l| l.contains("includes unresolved ExternalGem::Mixin"))
        .unwrap_or_else(|| panic!("{stderr}"));
    assert!(unowned.contains("gadget_tools.rb:2"), "{unowned}");
    assert!(!unowned.contains("(no location)"), "{unowned}");
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");

    let lowered = fs::read_to_string(out.join("vendor/futamura/lowered/app/models/gadget_tools.rb")).unwrap();
    assert!(lowered.contains("include ExternalGem::Mixin if defined?(ExternalGem::Mixin)"), "{lowered}");
    let ruby = Command::new("ruby")
        .arg(out.join("vendor/futamura/lowered/app/models/gadget_tools.rb"))
        .output()
        .expect("ruby must be on PATH");
    assert!(ruby.status.success(), "{}", String::from_utf8_lossy(&ruby.stderr));

    // Plain `--target ruby` leaves the same include bare.
    let out_ruby = scratch("inc-out-ruby");
    let run_ruby = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby", "--survey", "--allow-unsupported", "-o"])
        .arg(&out_ruby)
        .arg(&src)
        .output()
        .unwrap();
    assert!(run_ruby.status.success(), "{}", String::from_utf8_lossy(&run_ruby.stderr));
    let plain = fs::read_to_string(out_ruby.join("app/models/gadget_tools.rb")).unwrap();
    assert!(plain.contains("include ExternalGem::Mixin\n") && !plain.contains("if defined?"), "{plain}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
    fs::remove_dir_all(&out_ruby).unwrap();
}

/// `Rack::Utils::HTTP_STATUS_CODES` is a real Rack constant the lowered
/// runtime doesn't model: unresolved, a bare reference to it types as
/// `Var` (the same sentinel a totally unknown name gets), which trips
/// the Const arm's own "unresolved reference" diagnostic
/// (`analyze::body::compute`) wherever the app reads it — every route
/// that reaches the read, blocked directly, no fan-out needed. Typing
/// it as `Hash[Integer, String]` in the registry (matching Rack's own
/// table) resolves the constant, so the diagnostic — and the block —
/// disappears.
#[test]
fn http_status_codes_constant_resolves_and_the_route_clears() {
    let src = scratch("http-status-src");
    let out = scratch("http-status-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def index
    @reason = Rack::Utils::HTTP_STATUS_CODES[200]
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @reason %>\n");

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(!stderr.contains("HTTP_STATUS_CODES"), "{stderr}");
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// A method's own `&block` parameter (an `around_action` filter method
/// written `def with_x(&block); block.call; end`) used to stay untyped
/// through the controller→library lowering: `type_method_body` re-types
/// each method body from scratch and seeded only the signature's
/// params, never the method's own block param, so it read back as the
/// unresolved `Var` sentinel. Untyped, the planner's `block.call` fanned
/// out to every app method named `call`, including an unrelated class's
/// unsupported one. Fixed by seeding the block param the same way the
/// analyzer already does (`captured_block_ty`/the declared signature),
/// so `classes_of` answers "no app class" and no fan-out happens.
#[test]
fn a_methods_own_block_param_is_not_app_dispatch() {
    let src = scratch("blockcall-src");
    let out = scratch("blockcall-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", br#"class ApplicationController < ActionController::Base
  around_action :with_x

  def with_x(&block)
    block.call
  end
end
"#);
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def index
    @name = "hi"
  end
end
"#);
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");
    // Unrelated: its own `call`, with an unsupported body. Before the
    // fix, `block.call`'s untyped receiver fans out to this.
    write(&src, "app/lib/gadget.rb", br#"class Gadget
  def call
    Frobnicator::Spinner.new.spin
  end
end
"#);

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(!stderr.contains("Gadget") && !stderr.contains("Frobnicator"), "{stderr}");
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// INTERIM planner fallback (rubys/roundhouse#58, G2/G3): `routes::plan`
/// re-lowers every controller body from scratch
/// (`lower::typing::type_method_body`), which loses the analyzer's type
/// for a local bound to a framework helper's result (Devise's
/// `current_<scope>`) and for the app's own private helpers (pinned
/// `Untyped` on purpose by #810). Untyped, the planner's `enqueue_send`
/// fans a send out to every app method of that name — here, an unrelated
/// `Gadget`'s unsupported body — blocking routes that never actually
/// reach it. Three shapes of the same gap, all now covered by falling
/// back to the analyzer's own (closed) stamp for the receiver's span:
/// a local assigned from `current_user`, `current_user&.x` directly, and
/// a private helper's result used with `&.`. A fourth action keeps the
/// control: a receiver with no closed type on *either* side (a fully
/// dynamic `send`) must still fan out and stay blocked, so the fallback
/// never masks a genuinely unresolved call.
#[test]
fn planner_falls_back_to_the_analyzers_type_for_framework_and_private_helper_receivers() {
    let src = scratch("stamps-src");
    let out = scratch("stamps-out");
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", br#"Rails.application.routes.draw do
  get "widgets/a" => "widgets#action_a"
  get "widgets/b" => "widgets#action_b"
  get "widgets/c" => "widgets#action_c"
  get "widgets/d" => "widgets#action_d"
end
"#);
    write(&src, "db/schema.rb", br#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "email", null: false
  end
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#);
    write(&src, "app/models/application_record.rb", b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n");
    // Declaring `devise` makes `current_user` resolve to `User | nil`
    // in the analyzer's own registry (`analyze/registry/controllers.rs`).
    write(&src, "app/models/user.rb", b"class User < ApplicationRecord\n  devise :database_authenticatable, :registerable\nend\n");
    write(&src, "app/models/widget.rb", b"class Widget < ApplicationRecord\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(&src, "app/controllers/widgets_controller.rb", br#"class WidgetsController < ApplicationController
  def action_a
    u = current_user
    @email = u&.email
  end

  def action_b
    @email = current_user&.email
  end

  def action_c
    @name = current_widget&.name
  end

  def action_d
    target = [:current_widget].sample
    @thing = send(target)&.email
  end

  private

  def current_widget
    Widget.first
  end
end
"#);
    write(&src, "app/views/widgets/action_a.html.erb", b"<%= @email %>\n");
    write(&src, "app/views/widgets/action_b.html.erb", b"<%= @email %>\n");
    write(&src, "app/views/widgets/action_c.html.erb", b"<%= @name %>\n");
    write(&src, "app/views/widgets/action_d.html.erb", b"<%= @thing %>\n");
    // Unrelated: defines both receiver method names with an unsupported
    // body. Before the fix, every action's untyped receiver fans out
    // here; after, only the genuinely dynamic `action_d` still does.
    write(&src, "app/lib/gadget.rb", br#"class Gadget
  def email
    Frobnicator::Spinner.new.spin
  end

  def name
    Frobnicator::Spinner.new.spin
  end
end
"#);

    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(stderr.contains("whole-route: 3 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    for cleared in ["widgets#action_a", "widgets#action_b", "widgets#action_c"] {
        assert!(stderr.contains(&format!("lowered     {cleared}")), "{stderr}");
    }
    // The control: a fully dynamic `send` still has no closed type on
    // either side, so it still fans out to `Gadget` and stays blocked.
    let residue = stderr.lines().find(|l| l.contains("rails ") && l.contains("widgets#action_d")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(residue.contains("Gadget#email") && residue.contains("Frobnicator"), "{residue}");
    // None of the three cleared routes' call chains reach Gadget.
    for cleared in ["action_a", "action_b", "action_c"] {
        assert!(
            stderr.lines().find(|l| l.contains(cleared)).unwrap().matches("Gadget").count() == 0,
            "{stderr}"
        );
    }

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// The whole-route planner must reach every app method a route can run,
/// or it clears a route that runs unsupported code. Each fixture below
/// has one `widgets#index` route whose action reaches `Frobnicator`, a
/// constant the lowered app cannot resolve, through one call shape the
/// walk used to miss, and so must stay with Rails.
fn reach_ledger(name: &str, action: &str, helpers: &str, extra: &[(&str, &str)]) -> String {
    let src = scratch(&format!("{name}-src"));
    let out = scratch(&format!("{name}-out"));
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    let controller = format!(
        "class WidgetsController < ApplicationController\n{helpers}\n  def index\n{action}\n  end\n\n  private\n\n  def pick\n    Widget.new\n  end\nend\n"
    );
    write(&src, "app/controllers/widgets_controller.rb", controller.as_bytes());
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");
    write(&src, "app/lib/widget.rb", b"class Widget\nend\n");
    for (path, body) in extra {
        write(&src, path, body.as_bytes());
    }
    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "--survey", "-o"])
        .arg(&out)
        .arg(&src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(run.status.success(), "{stderr}");
    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
    stderr
}

const GADGET: (&str, &str) = ("app/lib/gadget.rb", "class Gadget\n  def call\n    Frobnicator::Spinner.new.spin\n  end\nend\n");

fn assert_left_to_rails(stderr: &str, via: &str) {
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    let residue = stderr.lines().find(|l| l.contains("rails ") && l.contains("widgets#index")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(residue.contains("Frobnicator") && residue.contains(via), "{residue}");
}

/// A record out of a relation is an app object: the walk used to skip
/// any send on a framework call's result (`ActiveRecord::Relation.new(
/// Post).where(…)…`, as the lowering spells it), and cleared the route.
#[test]
fn a_send_on_a_record_from_a_relation_is_followed() {
    let stderr = reach_ledger(
        "reach-relation",
        "    @name = Post.where(title: \"x\").to_a.sample.call",
        "",
        &[
            (
                "db/schema.rb",
                "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\", null: false\n  end\nend\n",
            ),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
            ("app/models/post.rb", "class Post < ApplicationRecord\n  def call\n    Frobnicator::Spinner.new.spin\n  end\nend\n"),
        ],
    );
    assert_left_to_rails(&stderr, "Post#call");
}

/// A value read back from the cache is whatever some request stored:
/// the walk used to skip the send because `Rails.cache` is the
/// framework's.
#[test]
fn a_send_on_a_value_from_the_cache_is_followed() {
    let stderr = reach_ledger("reach-cache", "    @name = Rails.cache.read(\"gadget\").call", "", &[GADGET]);
    assert_left_to_rails(&stderr, "Gadget#call");
}

/// A class-side `method_missing` answers a name no class method defines
/// (`Setting.some_key`).
#[test]
fn a_class_side_method_missing_is_followed() {
    let stderr = reach_ledger(
        "reach-cmissing",
        "    @name = Gizmo.anything",
        "",
        &[("app/lib/gizmo.rb", "class Gizmo\n  def self.method_missing(name, *args)\n    Frobnicator::Spinner.new.spin\n  end\nend\n")],
    );
    assert_left_to_rails(&stderr, "Gizmo.method_missing");
}

/// An instance-side `method_missing` answers an untyped send of a name
/// its class does not define. (A typed `Gizmo.new.anything` needs no
/// test here: the analysis already refuses it.)
#[test]
fn an_instance_side_method_missing_is_followed() {
    let stderr = reach_ledger(
        "reach-imissing",
        "    @name = Rails.cache.read(\"k\").anything",
        "",
        &[("app/lib/gizmo.rb", "class Gizmo\n  def method_missing(name, *args)\n    Frobnicator::Spinner.new.spin\n  end\nend\n")],
    );
    assert_left_to_rails(&stderr, "Gizmo#method_missing");
}

/// A typed value's class may answer a send directly on one side
/// (class or instance) while the other side has no definition of it
/// but does have its own `method_missing`. A hit on one side must not
/// suppress the other side's `method_missing` — each side is checked
/// on its own.
#[test]
fn a_hit_on_one_side_does_not_suppress_the_other_sides_method_missing() {
    // `foo` is an instance method, so `w.foo` is a direct hit on the
    // instance side; the class side has no `foo` but does have its own
    // `method_missing`. The instance hit must not suppress that check.
    // (`@name` stays a literal: rendering an untyped ivar would call
    // `to_s` and sweep in any class with an INSTANCE `method_missing`
    // through the unrelated general fan-out, masking this bug — this
    // repro needs the class-side one, which that fan-out never touches.)
    let stderr = reach_ledger(
        "reach-mixed-sides",
        "    w = Widget.new\n    w.foo\n    @name = \"ok\"",
        "",
        &[(
            "app/lib/widget.rb",
            "class Widget\n  def foo\n  end\n\n  def self.method_missing(name, *args)\n    Frobnicator::Spinner.new.spin\n  end\nend\n",
        )],
    );
    assert_left_to_rails(&stderr, "Widget.method_missing");
}

/// `"#{x}"` calls `x.to_s`.
#[test]
fn string_interpolation_calls_to_s() {
    // RTA bounds an untyped send to the route's live classes: `Gadget`
    // must actually be built on this route (not merely defined) for its
    // `to_s` to be a real dispatch target, so the action also builds one.
    let stderr = reach_ledger(
        "reach-interp",
        "    @other = Gadget.new\n    @name = \"<#{{ w: pick }[:w]}>\"",
        "",
        &[("app/lib/gadget.rb", "class Gadget\n  def to_s\n    Frobnicator::Spinner.new.spin\n  end\nend\n")],
    );
    assert_left_to_rails(&stderr, "Gadget#to_s");
}

/// `x.public_send(:call)` is `x.call`.
#[test]
fn public_send_with_a_literal_name_is_that_call() {
    let stderr = reach_ledger("reach-public-send", "    @name = Gadget.new.public_send(:call)", "", &[GADGET]);
    assert_left_to_rails(&stderr, "Gadget#call");
}

/// A constant holding an object, not a class: a send on it dispatches
/// on what it holds.
#[test]
fn a_send_on_a_value_constant_is_followed() {
    let stderr = reach_ledger("reach-value-const", "    @name = SPARE.call", "  SPARE = Gadget.new\n", &[GADGET]);
    assert_left_to_rails(&stderr, "Gadget#call");
}

/// A top-level value constant dispatches even when its own name also
/// matches a namespaced class by its last segment (`Admin::SPARE`):
/// the planner cannot tell which one a bare path like `SPARE` names,
/// so it has to reach through both, not just whichever it finds.
#[test]
fn a_value_constant_dispatches_despite_a_namespaced_class_of_the_same_last_segment() {
    // Referenced bare from the view (not from inside `WidgetsController`,
    // whose own scope would otherwise get prefixed onto the path ahead
    // of the match): the path stays plain `SPARE`, which also matches
    // `Admin::SPARE` by its last segment.
    let stderr = reach_ledger(
        "reach-value-const-collision",
        "    @name = \"ok\"",
        "  SPARE = Gadget.new\n",
        &[
            GADGET,
            ("app/lib/admin/spare.rb", "module Admin\n  class SPARE\n  end\nend\n"),
            ("app/views/widgets/index.html.erb", "<%= SPARE.call %>\n"),
        ],
    );
    assert_left_to_rails(&stderr, "Gadget#call");
}

/// A parameter's default value runs in the method.
#[test]
fn a_parameter_default_is_walked() {
    let stderr = reach_ledger(
        "reach-default",
        "    label\n    @name = \"hi\"",
        "  def label(text = Gadget.new.call)\n    text\n  end\n",
        &[GADGET],
    );
    assert_left_to_rails(&stderr, "Gadget#call");
}

/// Rapid type analysis: an untyped send reaches only the classes that
/// can have an instance while the route runs. Each fixture below sends
/// `call` from `widgets#index` to a value neither the lowering nor the
/// analyzer types (a helper's result read back out of a Hash literal),
/// and `Gadget#call`, on an unrelated plain class, has a body the
/// lowered app cannot run. Whether the route clears depends only on
/// whether a `Gadget` can exist on it.
fn rta_ledger(name: &str, action: &str, helpers: &str, extra: &[(&str, &str)]) -> String {
    rta_ledger_env(name, action, helpers, extra, &[])
}

/// `rta_ledger`, with extra environment variables set on the run (the
/// `ROUNDHOUSE_FUTAMURA_TRUST_BOX_CACHE` switch).
fn rta_ledger_env(name: &str, action: &str, helpers: &str, extra: &[(&str, &str)], envs: &[(&str, &str)]) -> String {
    let src = scratch(&format!("{name}-src"));
    let out = scratch(&format!("{name}-out"));
    write(&src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(&src, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"widgets\" => \"widgets#index\"\nend\n");
    write(&src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    let controller = format!(
        "class WidgetsController < ApplicationController\n  def index\n{action}\n  end\n\n  private\n\n  def pick\n    Widget.new\n  end\n{helpers}end\n"
    );
    write(&src, "app/controllers/widgets_controller.rb", controller.as_bytes());
    write(&src, "app/views/widgets/index.html.erb", b"<%= @name %>\n");
    write(&src, "app/lib/widget.rb", b"class Widget\n  def call\n    \"hi\"\n  end\nend\n");
    write(&src, "app/lib/gadget.rb", b"class Gadget\n  def call\n    Frobnicator::Spinner.new.spin\n  end\nend\n");
    for (path, body) in extra {
        write(&src, path, body.as_bytes());
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_roundhouse"));
    cmd.args(["--target", "futamura", "--survey", "-o"]).arg(&out).arg(&src).env("ROUNDHOUSE_FUTAMURA_LEDGER", "2");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let run = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(run.status.success(), "{stderr}");
    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
    stderr
}

/// Only a `Widget` is ever built, so the untyped `call` can only be
/// `Widget#call`: `Gadget#call` is not reached and the route clears.
#[test]
fn rta_untyped_send_reaches_only_classes_the_route_can_build() {
    let stderr = rta_ledger("rta-live", "    @name = { w: pick }[:w].call", "", &[]);
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");
    assert!(!stderr.contains("Frobnicator"), "{stderr}");
    assert!(stderr.contains("rta         widgets#index: bounded"), "{stderr}");
}

/// Control: the route also builds a `Gadget` (in a helper it calls), so
/// the same untyped `call` may be `Gadget#call`.
#[test]
fn rta_untyped_send_reaches_a_class_the_route_builds() {
    let stderr = rta_ledger(
        "rta-built",
        "    @name = { w: pick }[:w].call\n    @other = spare",
        "\n  def spare\n    Gadget.new\n  end\n",
        &[],
    );
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    assert!(stderr.contains("app/lib/gadget.rb:3") && stderr.contains("Gadget#call"), "{stderr}");
    assert!(stderr.contains("rta         widgets#index: bounded"), "{stderr}");
}

/// A record the database hands back is live. The runtime builds models
/// by name (its generated `instantiate-named` table), so every model is
/// live on every route, and a query's result is a framework value the
/// walk follows: the untyped `call` on a record out of a relation (the
/// lowering spells it `ActiveRecord::Relation.new(Post).where(…)`)
/// reaches `Post#call`. It does not reach `Gadget#call`: the route
/// reaches one error, not two.
#[test]
fn rta_a_record_from_a_finder_is_live() {
    let stderr = rta_ledger(
        "rta-model",
        "    @name = Post.where(title: \"x\").to_a.sample.call",
        "",
        &[
            (
                "db/schema.rb",
                "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\", null: false\n  end\nend\n",
            ),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
            ("app/models/post.rb", "class Post < ApplicationRecord\n  def call\n    Combobulator::Unit.new.run\n  end\nend\n"),
        ],
    );
    let residue = stderr.lines().find(|l| l.contains("rails ") && l.contains("widgets#index")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(residue.contains("reaches 1 error(s)") && residue.contains("app/models/post.rb:3"), "{residue}");
    assert!(!stderr.contains("Frobnicator"), "{stderr}");
}

/// A value read from the cache may have been written by another
/// request, by Rails code outside any box, or directly by an operator
/// (a console session, a migration) — the box's own reach can never
/// confirm or rule any of that out from the box's code alone. So by
/// default a cache read goes to full fan-out (case 5 in the design-doc
/// comment above `reach`), exactly like `constantize` or `Marshal.load`:
/// `Gadget` is never built anywhere in this fixture, yet it is still
/// reached.
#[test]
fn rta_cache_read_is_conservative_by_default() {
    let stderr = rta_ledger("rta-cache-default", "    @name = Rails.cache.read(\"gadget\").call", "", &[]);
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    assert!(stderr.contains("app/lib/gadget.rb:3"), "{stderr}");
    assert!(stderr.contains("rta         widgets#index: full fan-out: Rails.cache.read in WidgetsController#index"), "{stderr}");
}

/// `ROUNDHOUSE_FUTAMURA_TRUST_BOX_CACHE=1` opts into the narrower,
/// unconfirmed assumption that this box's cache only ever holds a value
/// some box code wrote: the read is then bounded to the app-wide live
/// set instead of fanning out by name. Nothing in this fixture ever
/// builds a `Gadget`, so under the switch the route actually clears —
/// the switch is not merely a different ledger label, it changes what
/// the route reaches.
#[test]
fn rta_cache_read_is_bounded_with_the_trust_switch() {
    let stderr = rta_ledger_env(
        "rta-cache-trust",
        "    @name = Rails.cache.read(\"gadget\").call",
        "",
        &[],
        &[("ROUNDHOUSE_FUTAMURA_TRUST_BOX_CACHE", "1")],
    );
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");
    assert!(!stderr.contains("Frobnicator"), "{stderr}");
    assert!(stderr.contains("rta         widgets#index: every app-live class: Rails.cache.read in WidgetsController#index"), "{stderr}");
}

/// The trust switch still catches a `Gadget` the app genuinely builds
/// (in a helper this route never calls): bounded is not the same as
/// unreached, only narrower than full fan-out.
#[test]
fn rta_cache_read_trust_switch_still_reaches_a_class_the_app_builds() {
    let stderr = rta_ledger_env(
        "rta-cache-trust-built",
        "    @name = Rails.cache.read(\"gadget\").call",
        "\n  def warm\n    Rails.cache.write(\"gadget\", Gadget.new)\n  end\n",
        &[],
        &[("ROUNDHOUSE_FUTAMURA_TRUST_BOX_CACHE", "1")],
    );
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    assert!(stderr.contains("app/lib/gadget.rb:3"), "{stderr}");
    assert!(stderr.contains("rta         widgets#index: every app-live class: Rails.cache.read in WidgetsController#index"), "{stderr}");
}

/// A class named by data can be any class, built or not: the route goes
/// back to full fan-out, and `Gadget#call` is reached though nothing in
/// the app ever builds a `Gadget`.
#[test]
fn rta_a_class_from_data_falls_back_to_full_fan_out() {
    let stderr = rta_ledger("rta-constantize", "    kind = params[:kind].constantize\n    @name = kind.new.call", "", &[]);
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    assert!(stderr.contains("app/lib/gadget.rb:3"), "{stderr}");
    assert!(stderr.contains("rta         widgets#index: full fan-out: `constantize` in WidgetsController#index"), "{stderr}");
}

/// A registry the app fills at load time, read back on the route
/// (`Registry.items.first`): nothing in the route names the class
/// whose object it holds.
const REGISTRY: (&str, &str) = (
    "app/lib/registry.rb",
    "class Registry\n  ITEMS = []\n\n  def self.items\n    ITEMS\n  end\nend\n",
);

fn registering_gadget(arg: &str) -> String {
    format!(
        "class Gadget\n  def self.register(x)\n    Registry::ITEMS << x\n  end\n  register({arg})\n\n  def call\n    Frobnicator::Spinner.new.spin\n  end\nend\n"
    )
}

/// A class body runs with the class as `self`: `register(new)` builds
/// a `Gadget` at load time and hands it on, written receiverless or on
/// `self`, with `new` or `allocate`. The `Gadget` is live, so the
/// untyped `call` on what the registry holds reaches `Gadget#call`.
#[test]
fn rta_a_class_body_that_builds_its_own_instance_makes_it_live() {
    for arg in ["new", "allocate", "self.new", "self.allocate"] {
        let gadget = registering_gadget(arg);
        let stderr = rta_ledger(
            &format!("rta-body-{}", arg.replace('.', "-")),
            "    @name = Registry.items.first.call",
            "",
            &[REGISTRY, ("app/lib/gadget.rb", &gadget)],
        );
        assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{arg}: {stderr}");
        assert!(stderr.contains("app/lib/gadget.rb:8") && stderr.contains("Gadget#call"), "{arg}: {stderr}");
    }
}

/// `register(self)` hands on the class object itself: whoever holds it
/// can build one (`Registry.items.first.new.call`).
#[test]
fn rta_a_class_body_that_hands_on_self_makes_the_class_live() {
    let gadget = registering_gadget("self");
    let stderr = rta_ledger(
        "rta-body-self",
        "    @name = Registry.items.first.new.call",
        "",
        &[REGISTRY, ("app/lib/gadget.rb", &gadget)],
    );
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    assert!(stderr.contains("app/lib/gadget.rb:8") && stderr.contains("Gadget#call"), "{stderr}");
}

/// Precision: a class body that only makes framework calls (`include`,
/// `attr_reader`) hands nothing on, so `Gadget` stays dead and the
/// untyped `call` on a `Widget` still clears.
#[test]
fn rta_a_class_body_of_framework_calls_keeps_the_class_dead() {
    let stderr = rta_ledger(
        "rta-body-framework",
        "    @name = { w: pick }[:w].call",
        "",
        &[(
            "app/lib/gadget.rb",
            "class Gadget\n  include Comparable\n  attr_reader :size\n\n  def call\n    Frobnicator::Spinner.new.spin\n  end\nend\n",
        )],
    );
    assert!(stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 0 left to Rails"), "{stderr}");
    assert!(!stderr.contains("Frobnicator"), "{stderr}");
}

/// Ruby passes each subclass to its parent's `inherited` hook when the
/// subclass loads, with nothing in the subclass' body to show for it:
/// every class under an app `inherited` hook is live.
#[test]
fn rta_an_inherited_hook_makes_every_subclass_live() {
    let stderr = rta_ledger(
        "rta-inherited",
        "    @name = Registry.items.first.new.call",
        "",
        &[
            REGISTRY,
            ("app/lib/base.rb", "class Base\n  def self.inherited(k)\n    Registry::ITEMS << k\n  end\nend\n"),
            ("app/lib/gadget.rb", "class Gadget < Base\n  def call\n    Frobnicator::Spinner.new.spin\n  end\nend\n"),
        ],
    );
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
    assert!(stderr.contains("app/lib/gadget.rb:3") && stderr.contains("Gadget#call"), "{stderr}");
}

/// `Gadget` gets into the registry through a module, with nothing in
/// its own body but `include` / `extend`: the module's `included` /
/// `extended` hook, or an extended module's method that the class body
/// calls. Each makes `Gadget` live.
#[test]
fn rta_a_module_that_hands_on_its_includer_makes_it_live() {
    let cases = [
        (
            "included-hook",
            "module Registrable\n  def self.included(base)\n    Registry::ITEMS << base\n  end\nend\n",
            "include Registrable",
        ),
        (
            "extended-hook",
            "module Registrable\n  def self.extended(base)\n    Registry::ITEMS << base\n  end\nend\n",
            "extend Registrable",
        ),
        (
            "extended-method",
            "module Registrable\n  def register\n    Registry::ITEMS << self\n  end\nend\n",
            "extend Registrable\n  register",
        ),
    ];
    let mut failed = Vec::new();
    for (name, module, body) in cases {
        let gadget = format!("class Gadget\n  {body}\n\n  def call\n    Frobnicator::Spinner.new.spin\n  end\nend\n");
        let stderr = rta_ledger(
            &format!("rta-module-{name}"),
            "    @name = Registry.items.first.new.call",
            "",
            &[REGISTRY, ("app/lib/registrable.rb", module), ("app/lib/gadget.rb", &gadget)],
        );
        if !(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails") && stderr.contains("Gadget#call")) {
            failed.push(format!("{name}: {stderr}"));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// A class-side `method_missing` answers a name no class defines
/// (`Setting.some_key`): the walk follows it, so an error behind it
/// blocks the route. (This is already covered independently of RTA's
/// bounding — see `a_class_side_method_missing_is_followed` above —
/// but is ported here too since it is one of RTA's own 6 tests.)
#[test]
fn a_class_side_method_missing_is_reached() {
    let stderr = rta_ledger(
        "rta-missing",
        "    @name = Gizmo.anything",
        "",
        &[("app/lib/gizmo.rb", "class Gizmo\n  def self.method_missing(name, *args)\n    Frobnicator::Spinner.new.spin\n  end\nend\n")],
    );
    assert!(stderr.contains("app/lib/gizmo.rb:3") && stderr.contains("Gizmo.method_missing"), "{stderr}");
    assert!(stderr.contains("whole-route: 0 GET action(s) served by the lowered app; 1 left to Rails"), "{stderr}");
}

/// A minimal app whose `ruby` emit refuses one association (a `through:`
/// hop keyed on a non-id column, Mastodon's `Status.reblogged_by_accounts`).
fn app_with_a_ruby_refusal(src: &Path) {
    write(src, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(src, "db/schema.rb", br#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "posts", force: :cascade do |t|
    t.string "title", null: false
    t.integer "user_id", null: false
    t.integer "reblog_of_id"
  end
end
"#);
    write(src, "config/routes.rb", b"Rails.application.routes.draw do\n  resources :posts, only: [:index]\nend\n");
    write(src, "app/models/application_record.rb", b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n");
    write(src, "app/models/user.rb", b"class User < ApplicationRecord\nend\n");
    write(src, "app/models/post.rb", br#"class Post < ApplicationRecord
  belongs_to :user
  has_many :reblogs, foreign_key: "reblog_of_id", class_name: "Post"
  has_many :reblogged_by_users, through: :reblogs, class_name: "User", source: :user
end
"#);
    write(src, "app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base\nend\n");
    write(src, "app/controllers/posts_controller.rb", b"class PostsController < ApplicationController\n  def index\n    @posts = Post.all\n  end\nend\n");
    write(src, "app/views/posts/index.html.erb", b"<% @posts.each do |post| %><p><%= post.title %></p><% end %>\n");
}

/// The `ruby` emit's errors decide which routes stay on Rails; they are
/// not futamura's own, and must not fail its emit (Mastodon on 9de9af1e
/// exited 1 on `Status.reblogged_by_accounts`).
#[test]
fn ruby_emit_refusals_do_not_fail_the_futamura_emit() {
    let src = scratch("leak-src");
    let out = scratch("leak-out");
    app_with_a_ruby_refusal(&src);
    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(!stderr.contains("association_primary_key"), "{stderr}");
}

mod query_specialization {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use roundhouse::analyze::Analyzer;
    use roundhouse::emit::futamura::{INITIALIZER, specialize};
    use roundhouse::ingest::ingest_app_from_tree;

    const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.integer "room_id", null: false
    t.integer "creator_id", null: false
    t.boolean "pinned", default: false
    t.datetime "created_at", null: false
  end
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

    const MESSAGE: &str = r#"class Message < ApplicationRecord
  PAGE_SIZE = 40

  belongs_to :room
  belongs_to :creator, class_name: "User"

  scope :ordered, -> { order(:created_at) }
  scope :with_creator, -> { preload(:creator) }
  scope :last_page, -> { ordered.last(PAGE_SIZE) }
  scope :before, ->(message) { where("created_at < ?", message.created_at) }
end
"#;

    const CONTROLLER: &str = r#"class MessagesController < ApplicationController
  def index
    @room = Room.find(params[:room_id])
    @messages = @room.messages.with_creator.last_page
    @pinned = Message.where(room_id: @room.id, pinned: true).order(created_at: :desc).first(3)
    @older = @room.messages.before(@messages.first).to_a
  end
end
"#;

    fn app_and_files() -> (roundhouse::App, Vec<(String, String)>) {
        let files: Vec<(&str, &str)> = vec![
            ("db/schema.rb", SCHEMA),
            ("app/models/room.rb", "class Room < ApplicationRecord\n  has_many :messages\nend\n"),
            ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
            ("app/models/message.rb", MESSAGE),
            ("app/controllers/messages_controller.rb", CONTROLLER),
        ];
        let tree: HashMap<PathBuf, Vec<u8>> =
            files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        Analyzer::new(&app).analyze(&mut app);
        (app, files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect())
    }

    fn file<'f>(files: &'f [(String, String)], path: &str) -> &'f str {
        &files.iter().find(|(p, _)| p == path).unwrap_or_else(|| panic!("{path} not emitted")).1
    }

    #[test]
    fn association_chain_through_scopes_becomes_a_statement_with_its_owner_bound() {
        let (app, mut files) = app_and_files();
        let report = specialize(&app, &mut files).unwrap();
        let controller = file(&files, "app/controllers/messages_controller.rb");
        let init = file(&files, INITIALIZER);
        // The call site binds the owner's id and keeps the chain as fallback.
        assert!(controller.contains(
            "@messages = Futamura.records(:controllers_messages_controller_4_17, [@room.id], owner: @room, \
             association: :messages) { @room.messages.with_creator.last_page }"
        ), "{controller}");
        // Static scopes stay calls for Rails to build; the terminal scope is
        // inlined, its constant qualified so the initializer can name it,
        // and `last(n)` follows Rails' FinderMethods: ordered_relation,
        // limit, reverse_order, then the rows reversed.
        assert!(init.contains(
            "Futamura.define(:controllers_messages_controller_4_17, Message, reverse: true) \
             { |p| Message.all.where(room_id: p.bind).with_creator.ordered.send(:ordered_relation).limit(Message::PAGE_SIZE).reverse_order }"
        ), "{init}");
        assert!(report.specialized.iter().any(|s| s.ends_with("messages_controller.rb:4:17")), "{:?}", report.specialized);
    }

    #[test]
    fn constant_rooted_chain_binds_request_values_and_bakes_literals() {
        let (app, mut files) = app_and_files();
        specialize(&app, &mut files).unwrap();
        let controller = file(&files, "app/controllers/messages_controller.rb");
        let init = file(&files, INITIALIZER);
        assert!(controller.contains("@pinned = Futamura.records(:controllers_messages_controller_5_15, [@room.id]) {"), "{controller}");
        assert!(init.contains(
            "Message.all.where(room_id: p.bind, pinned: true).order({ created_at: :desc }).send(:ordered_relation).limit(3) }"
        ), "{init}");
    }

    #[test]
    fn a_string_condition_over_a_record_is_left_to_rails_with_a_reason() {
        let (app, mut files) = app_and_files();
        let report = specialize(&app, &mut files).unwrap();
        let controller = file(&files, "app/controllers/messages_controller.rb");
        assert!(controller.contains("@older = @room.messages.before(@messages.first).to_a\n"), "{controller}");
        assert!(report.residue.iter().any(|r| r.contains("messages_controller.rb:6:")), "{:?}", report.residue);
    }
}

mod single_record_terminals {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use roundhouse::analyze::Analyzer;
    use roundhouse::emit::futamura::{INITIALIZER, specialize};
    use roundhouse::ingest::ingest_app_from_tree;

    #[test]
    fn first_without_order_takes_rails_implicit_order_and_find_by_is_where_take() {
        let schema = "ActiveRecord::Schema.define do\n  create_table \"stories\", force: :cascade do |t|\n    t.string \"short_id\", null: false\n  end\nend\n";
        let controller = "class StoriesController < ApplicationController\n  def show\n    @story = Story.where(short_id: params[:id].to_s).first\n    @same = Story.find_by(short_id: params[:id] || params[:story_id])\n  end\nend\n";
        let files: Vec<(&str, &str)> = vec![
            ("db/schema.rb", schema),
            ("app/models/story.rb", "class Story < ApplicationRecord\nend\n"),
            ("app/controllers/stories_controller.rb", controller),
        ];
        let tree: HashMap<PathBuf, Vec<u8>> =
            files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        Analyzer::new(&app).analyze(&mut app);
        let mut files: Vec<(String, String)> = files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect();
        specialize(&app, &mut files).unwrap();
        let get = |path: &str| files.iter().find(|(p, _)| p == path).unwrap().1.clone();
        let init = get(INITIALIZER);
        let out = get("app/controllers/stories_controller.rb");
        assert!(init.contains("Story, one: true) { |p| Story.all.where(short_id: p.bind).send(:ordered_relation).limit(1) }"), "{init}");
        assert!(init.contains("Story, one: true) { |p| Story.all.where(short_id: p.bind).limit(1) }"), "{init}");
        assert!(out.contains("[params[:id].to_s]) { Story.where(short_id: params[:id].to_s).first }"), "{out}");
        assert!(out.contains("[params[:id] || params[:story_id]]) {"), "{out}");
    }
}

mod preload_scopes {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use roundhouse::analyze::Analyzer;
    use roundhouse::emit::futamura::{INITIALIZER, specialize};
    use roundhouse::ingest::ingest_app_from_tree;

    /// The runtime preloader builds an association's scope once, so only a
    /// scope Roundhouse can see is built from literals is certified to it.
    #[test]
    fn literal_association_scopes_are_certified_and_request_dependent_ones_are_not() {
        let schema = "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"posts\", force: :cascade do |t|\n    t.integer \"user_id\"\n    t.boolean \"published\"\n    t.integer \"viewer_id\"\n  end\nend\n";
        let user = "class User < ApplicationRecord\n  has_many :posts\n  has_many :published_posts, -> { where(published: true).order(:id) }, class_name: \"Post\"\n  has_many :seen_posts, -> { where(viewer_id: Current.user.id) }, class_name: \"Post\"\nend\n";
        let controller = "class UsersController < ApplicationController\n  def index\n    @users = User.preload(:published_posts).order(:id).first(10)\n  end\nend\n";
        let files: Vec<(&str, &str)> = vec![
            ("db/schema.rb", schema),
            ("app/models/user.rb", user),
            ("app/models/post.rb", "class Post < ApplicationRecord\n  belongs_to :user\nend\n"),
            ("app/controllers/users_controller.rb", controller),
        ];
        let tree: HashMap<PathBuf, Vec<u8>> =
            files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        Analyzer::new(&app).analyze(&mut app);
        let mut files: Vec<(String, String)> = files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect();
        specialize(&app, &mut files).unwrap();
        let init = &files.iter().find(|(p, _)| p == INITIALIZER).expect("initializer").1;
        assert!(init.contains("Futamura::Preload.static_scopes(\"User#published_posts\")\n"), "{init}");
        assert!(!init.contains("User#seen_posts"), "{init}");
    }
}
