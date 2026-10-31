//! A template and a partial sharing a base name in one directory
//! (`articles/show.html.erb` beside `articles/_show.html.erb`) are two
//! templates in Rails: `render "show"` names the partial, an action's
//! render names the template. Both used to lower to `Views::Articles.show`
//! — two definitions of one method, which the spinel build refuses as a
//! twice-declared signature and CRuby settles by letting the later-loaded
//! file replace the other.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const PARTIAL: &str = "<p id=\"show-partial\">Partial shows <%= article.title %></p>\n";

const HEADING: &str = "  <h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n";

/// The show page with `<%= render "show", article: @article %>` under
/// its heading, rendering its own-named partial.
fn show_renders_its_partial() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/views/articles/_show.html.erb", PARTIAL)
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!("{HEADING}  <%= render \"show\", article: @article %>\n"),
        )
}

/// Every `def self.<name>` the emitted files of `dir` declare, with the
/// file declaring it.
fn declared_methods(emitted: &std::path::Path, dir: &str, ext: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(emitted.join(dir)).expect("emitted view dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some(ext) {
            continue;
        }
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        for line in std::fs::read_to_string(&path).expect("emitted file").lines() {
            let Some(rest) = line.trim_start().strip_prefix("def self.") else { continue };
            let name: String =
                rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            out.push((name, file.clone()));
        }
    }
    out
}

fn duplicates(methods: &[(String, String)]) -> Vec<String> {
    let mut seen = std::collections::HashMap::new();
    let mut dups = Vec::new();
    for (name, file) in methods {
        if let Some(first) = seen.insert(name.clone(), file.clone()) {
            dups.push(format!("{name} in {first} and {file}"));
        }
    }
    dups
}

#[test]
fn a_template_rendering_its_same_named_partial_renders_both() {
    show_renders_its_partial()
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h2\", \"Comments\"\n",
            "    assert_select \"h2\", \"Comments\"\n    \
             assert_match(/Getting Started with Rails<\\/h1>\\s*<p id=\"show-partial\">Partial shows Getting Started with Rails<\\/p>/, response.body)\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// `render "show"` in `show.turbo_stream.erb` looks up partials in
/// turbo_stream, then html, and finds the html `_show` — the partial,
/// not the html template of its name.
#[test]
fn a_turbo_stream_template_rendering_its_same_named_partial_renders_the_partial() {
    show_renders_its_partial()
        .write("app/views/articles/show.turbo_stream.erb", "<%= render \"show\", article: @article %>")
        .edit(
            "test/controllers/articles_controller_test.rb",
            "  test \"should get edit\" do\n",
            "  test \"should show article as a turbo stream\" do\n    \
             get article_url(@article, format: :turbo_stream)\n    \
             assert_response :success\n    \
             assert_equal \"<p id=\\\"show-partial\\\">Partial shows Getting Started with Rails</p>\\n\", response.body\n  end\n\n  \
             test \"should get edit\" do\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// The partial's other callers — another template naming it by its full
/// path, and a controller's `render partial:` — reach the partial too,
/// not the template it shares a name with.
#[test]
fn other_renders_of_the_partial_reach_the_partial() {
    show_renders_its_partial()
        .edit(
            "app/views/articles/edit.html.erb",
            "  <h1 class=\"font-bold text-4xl\">Editing article</h1>\n",
            "  <h1 class=\"font-bold text-4xl\">Editing article</h1>\n  <%= render \"articles/show\", article: @article %>\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"articles/:id/preview\", to: \"articles#preview\", as: :preview_article\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  # GET /articles/new\n",
            "  def preview\n    @article = Article.find(params.expect(:id))\n    \
             render partial: \"show\", locals: { article: @article }\n  end\n\n  # GET /articles/new\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "  test \"should get edit\" do\n    get edit_article_url(@article)\n    assert_response :success\n",
            "  test \"should preview article\" do\n    get preview_article_url(@article)\n    assert_response :success\n    \
             assert_equal \"<p id=\\\"show-partial\\\">Partial shows Getting Started with Rails</p>\\n\", response.body\n  end\n\n  \
             test \"should get edit\" do\n    get edit_article_url(@article)\n    assert_response :success\n    \
             assert_match(/Editing article<\\/h1>\\s*<p id=\"show-partial\">Partial shows Getting Started with Rails<\\/p>/, response.body)\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// A `cached: true` collection of the partial renders the partial for
/// each element, as the uncached collection does.
#[test]
fn a_cached_collection_of_the_partial_renders_the_partial() {
    show_renders_its_partial()
        .edit(
            "app/views/articles/index.html.erb",
            "      <%= render @articles %>\n",
            "      <%= render partial: \"show\", collection: @articles, as: :article, cached: true %>\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "      assert_select \"h2\", minimum: 1\n",
            "      assert_select \"#show-partial\", \"Partial shows Getting Started with Rails\"\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// The spinel target declares each method once, in both the Ruby it
/// compiles and the `.rbs` sidecars it checks that Ruby against.
#[test]
fn the_spinel_target_declares_the_template_and_the_partial_apart() {
    let (emitted, errors) = show_renders_its_partial().emit(BuildTarget::Spinel);
    assert_eq!(errors, Vec::<String>::new(), "the spinel emit reports no errors");
    for ext in ["rb", "rbs"] {
        let methods = declared_methods(&emitted, "app/views/articles", ext);
        assert!(
            methods.iter().any(|(m, f)| m == "show_into" && f == &format!("show.{ext}")),
            "the template keeps its name: {methods:?}"
        );
        assert!(
            methods.iter().any(|(m, f)| m.ends_with("_into") && f == &format!("_show.{ext}")),
            "the partial is emitted: {methods:?}"
        );
        assert_eq!(duplicates(&methods), Vec::<String>::new(), "one declaration per method (.{ext})");
    }
}

/// The jbuilder pair: `show.json.jbuilder` merging in `_show.json.jbuilder`
/// — both were `show_json`.
#[test]
fn a_jbuilder_template_rendering_its_same_named_partial_renders_both() {
    emit_and_run::real_blog()
        .write("app/views/articles/_show.json.jbuilder", "json.headline article.title\n")
        .edit(
            "app/views/articles/show.json.jbuilder",
            "json.partial! \"articles/article\", article: @article\n",
            "json.partial! \"articles/article\", article: @article\njson.partial! \"show\", article: @article\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "  test \"should get edit\" do\n",
            "  test \"should show article as json\" do\n    get article_url(@article, format: :json)\n    \
             assert_response :success\n    \
             assert_includes response.body, \"\\\"title\\\":\\\"Getting Started with Rails\\\"\"\n    \
             assert_includes response.body, \"\\\"headline\\\":\\\"Getting Started with Rails\\\"\"\n  end\n\n  \
             test \"should get edit\" do\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// `new` is the one stem the template method is prefixed for (`_new`, so
/// it is not `Object.new`) — which is also how a partial `_new` spells.
/// The partial must take a name neither of them has.
#[test]
fn a_new_partial_beside_the_new_template_renders_apart() {
    emit_and_run::real_blog()
        .write("app/views/articles/_new.html.erb", "<p id=\"new-partial\">Start writing</p>\n")
        .edit(
            "app/views/articles/new.html.erb",
            "  <h1 class=\"font-bold text-4xl\">New article</h1>\n",
            "  <h1 class=\"font-bold text-4xl\">New article</h1>\n  <%= render \"new\" %>\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    get new_article_url\n    assert_response :success\n",
            "    get new_article_url\n    assert_response :success\n    \
             assert_match(/New article<\\/h1>\\s*<p id=\"new-partial\">Start writing<\\/p>/, response.body)\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// The renamed partial must not land on a name the directory already
/// defines: a template `show_partial` and its partial, the `_into`
/// variant of a template (`show_partial_partial_into`), and a jbuilder
/// format (`show_partial_partial_partial_json`). Each partial takes a
/// name of its own, and the second does not reuse the first's.
#[test]
fn a_renamed_partial_takes_a_name_no_template_or_earlier_partial_has() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/views/articles/_show.html.erb", "<p>a</p>\n")
        .write("app/views/articles/_show.json.jbuilder", "json.headline article.title\n")
        .write("app/views/articles/show_partial.html.erb", "<p>b</p>\n")
        .write("app/views/articles/_show_partial.html.erb", "<p>c</p>\n")
        .write("app/views/articles/show_partial_partial_into.html.erb", "<p>d</p>\n")
        .write("app/views/articles/show_partial_partial_partial_json.html.erb", "<p>e</p>\n")
        .emit(BuildTarget::Ruby);
    assert_eq!(errors, Vec::<String>::new(), "the emit reports no errors");
    let methods = declared_methods(&emitted, "app/views/articles", "rb");
    assert_eq!(duplicates(&methods), Vec::<String>::new(), "one declaration per method: {methods:?}");
    for file in ["_show.rb", "_show_json.rb", "_show_partial.rb"] {
        assert!(methods.iter().any(|(_, f)| f == file), "{file} is emitted: {methods:?}");
    }
}

/// A partial at the top of `app/views` beside a template of its name:
/// the pair has no graph key, and still defines two methods.
#[test]
fn a_top_level_pair_defines_two_methods() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/views/about.html.erb", "<h1>About</h1>\n")
        .write("app/views/_about.html.erb", "<p>About us</p>\n")
        .emit(BuildTarget::Ruby);
    assert_eq!(errors, Vec::<String>::new(), "the emit reports no errors");
    let methods = declared_methods(&emitted, "app/views", "rb");
    assert!(methods.iter().any(|(m, f)| m == "about" && f == "about.rb"), "{methods:?}");
    assert!(methods.iter().any(|(m, f)| m == "about_partial" && f == "_about.rb"), "{methods:?}");
    assert_eq!(duplicates(&methods), Vec::<String>::new(), "one declaration per method");
}

/// A template `articles/article` beside the record partial
/// `articles/_article`: the broadcasts that render a record's own
/// partial — `broadcasts_to` on Article and `article.broadcast_replace_to`
/// in Comment's commit callback — reach the partial, not the template.
#[test]
fn default_broadcasts_render_the_record_partial_beside_a_same_named_template() {
    broadcasting_blog()
        .write(
            "test/models/article_broadcast_test.rb",
            "require \"test_helper\"\n\n\
             class ArticleBroadcastTest < ActiveSupport::TestCase\n  \
             test \"the broadcasts render the article partial\" do\n    \
             Broadcasts.reset_log!\n    \
             article = Article.create!(title: \"Broadcast me\", body: \"A body long enough to pass.\")\n    \
             article.comments.create!(commenter: \"Ann\", body: \"Hello\")\n    \
             sent = Broadcasts.log.select { |entry| entry[:stream] == \"articles\" }\n    \
             assert_equal [:prepend, :replace], sent.map { |entry| entry[:action] }\n    \
             sent.each do |entry|\n      \
             assert_includes entry[:html], \"id=\\\"article_#{article.id}\\\"\"\n      \
             assert_not_includes entry[:html], \"Not the partial\"\n    \
             end\n  \
             end\nend\n",
        )
        .run_test("test/models/article_broadcast_test.rb")
        .assert_passes();
}

/// A controller's `@article.broadcast_replace_to` with no payload renders
/// the record's partial too.
#[test]
fn a_controller_broadcast_renders_the_record_partial_beside_a_same_named_template() {
    broadcasting_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"articles/:id/announce\", to: \"articles#announce\", as: :announce_article\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  # GET /articles/new\n",
            "  def announce\n    @article = Article.find(params.expect(:id))\n    \
             @article.broadcast_replace_to :articles\n    render plain: \"ok\"\n  end\n\n  # GET /articles/new\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "  test \"should get edit\" do\n",
            "  test \"should announce article\" do\n    \
             get announce_article_url(@article)\n    \
             assert_response :success\n    \
             html = Broadcasts.log.last[:html]\n    \
             assert_includes html, \"id=\\\"article_#{@article.id}\\\"\"\n    \
             assert_not_includes html, \"Not the partial\"\n  end\n\n  \
             test \"should get edit\" do\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// A helper's `render "articles/show"` returns the partial, not the
/// template of its name.
#[test]
fn a_helper_rendering_the_partial_reaches_the_partial() {
    show_renders_its_partial()
        .edit(
            "app/views/articles/show.html.erb",
            "  <%= render \"show\", article: @article %>\n",
            "  <%= show_card(@article) %>\n",
        )
        .write(
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  def show_card(article)\n    render \"articles/show\", article: article\n  end\nend\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h2\", \"Comments\"\n",
            "    assert_select \"h2\", \"Comments\"\n    \
             assert_match(/Getting Started with Rails<\\/h1>\\s*<p id=\"show-partial\">Partial shows Getting Started with Rails<\\/p>/, response.body)\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// The blog with a template `articles/article` beside the record partial
/// `articles/_article` its models broadcast.
fn broadcasting_blog() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/views/articles/article.html.erb", "<h1>Not the partial</h1>\n")
}

/// Every target lowers those broadcasts from the app's views: the
/// TypeScript and Crystal models call the partial's method too.
#[test]
fn default_broadcasts_call_the_record_partial_on_every_target() {
    for (target, call) in [
        (BuildTarget::Typescript, "Views.Articles.article"),
        (BuildTarget::Crystal, "Views::Articles.article"),
    ] {
        let (emitted, errors) = broadcasting_blog().emit(target);
        assert_eq!(errors, Vec::<String>::new(), "the {target:?} emit reports no errors");
        let models: Vec<String> = walk_files(&emitted)
            .into_iter()
            .filter(|p| p.contains("/models/"))
            .map(|p| std::fs::read_to_string(&p).expect("emitted model"))
            .collect();
        let partial_calls: usize =
            models.iter().map(|m| m.matches(&format!("{call}_partial(")).count()).sum();
        assert_eq!(partial_calls, 4, "{target:?}: create, update and both comment callbacks");
        assert!(
            !models.iter().any(|m| m.contains(&format!("{call}("))),
            "{target:?} calls the template:\n{}",
            models.join("\n")
        );
    }
}

/// Every file under `dir`, recursively.
fn walk_files(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("emitted dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else {
            out.push(path.to_string_lossy().into_owned());
        }
    }
    out
}

/// `turbo_stream.replace …, partial:` renders the partial through the
/// same call a `render` does, so the template has to thread the ivars
/// the partial reads — here `@heading`, which the show template never
/// reads itself. (The index renders the partial too, with a plain
/// `render`, which is where the analyzer learns `@heading`'s type.)
#[test]
fn a_turbo_stream_partial_render_threads_the_partials_ivars() {
    emit_and_run::real_blog()
        .write("app/views/articles/_show.html.erb", "<p id=\"heading\"><%= @heading %></p>\n")
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!("{HEADING}  <%= turbo_stream.replace \"article-heading\", partial: \"show\" %>\n"),
        )
        .edit(
            "app/views/articles/index.html.erb",
            "    <h1 class=\"font-bold text-4xl\">Articles</h1>\n",
            "    <h1 class=\"font-bold text-4xl\">Articles</h1>\n    <%= render \"show\" %>\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  def show\n  end\n",
            "  def show\n    @heading = \"Read on\"\n  end\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @heading = \"All of them\"\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h2\", \"Comments\"\n",
            "    assert_select \"h2\", \"Comments\"\n    \
             assert_includes response.body, \"Read on\"\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Action Text's generated attachment dispatch calls an attachable's
/// partial by the method it lowered to: `users/_mention` beside a
/// `users/mention` template is `Views::Users.mention_partial`.
#[test]
fn the_attachable_dispatch_calls_the_renamed_partial() {
    let tree = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :name\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include ActionText::Attachable\n\n  \
             def to_attachable_partial_path\n    \"users/mention\"\n  end\nend\n",
        ),
        ("app/views/users/_mention.html.erb", "<span><%= user.name %></span>\n"),
        ("app/views/users/mention.html.erb", "<h1>Mentions</h1>\n"),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, roundhouse::fixtures::real_blog())
        .expect("spinel tree");
    let action_text = files
        .iter()
        .find(|(p, _)| p.ends_with("runtime/action_text.rb"))
        .map(|(_, c)| c.clone())
        .expect("runtime/action_text.rb in the tree");
    assert!(action_text.contains("Views::Users.mention_partial("), "{action_text}");
    assert!(!action_text.contains("Views::Users.mention("), "{action_text}");
}

/// `product-item/` and `product_item/` are one module,
/// `Views::ProductItem`: a template in one and a partial of its name in
/// the other are a pair, and the partial's new name has to be clear of
/// the templates in both.
#[test]
fn directories_of_one_module_share_their_names() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/views/product-item/show.html.erb", "<h1>Item</h1>\n")
        .write("app/views/product_item/_show.html.erb", "<p>Item partial</p>\n")
        .write("app/views/product_item/show_partial.html.erb", "<p>Taken</p>\n")
        .emit(BuildTarget::Ruby);
    assert_eq!(errors, Vec::<String>::new(), "the emit reports no errors");
    let mut methods = declared_methods(&emitted, "app/views/product-item", "rb");
    methods.extend(declared_methods(&emitted, "app/views/product_item", "rb"));
    assert!(
        methods.iter().any(|(m, f)| m == "show_partial_partial" && f == "_show.rb"),
        "{methods:?}"
    );
    assert_eq!(duplicates(&methods), Vec::<String>::new(), "one declaration per method");
}

/// A strict-locals partial taking a keyword local has no `_into`
/// variant, so a template that happens to be named like one is no pair:
/// `_card` keeps `card` beside `card_into.html.erb`.
#[test]
fn a_keyword_strict_locals_partial_has_no_buffer_variant_to_collide() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/views/articles/_card.html.erb",
            "<%# locals: (article:, label: \"Read\") -%>\n<p><%= label %> <%= article.title %></p>\n",
        )
        .write("app/views/articles/card_into.html.erb", "<p>Into</p>\n")
        .emit(BuildTarget::Ruby);
    assert_eq!(errors, Vec::<String>::new(), "the emit reports no errors");
    let methods = declared_methods(&emitted, "app/views/articles", "rb");
    assert!(methods.iter().any(|(m, f)| m == "card" && f == "_card.rb"), "{methods:?}");
    assert!(!methods.iter().any(|(m, _)| m.starts_with("card_partial")), "{methods:?}");
    assert_eq!(duplicates(&methods), Vec::<String>::new(), "one declaration per method");
}

/// The name a pair's partial is given can be one an app helper already
/// has: `_show` becomes `show_partial`, and calls
/// `ApplicationHelper#show_partial`. The bare call in the template is
/// still the helper — a template is no method that could call itself.
#[test]
fn a_renamed_partial_still_calls_the_helper_of_its_new_name() {
    show_renders_its_partial()
        .write(
            "app/views/articles/_show.html.erb",
            "<p id=\"show-partial\"><%= show_partial(article) %></p>\n",
        )
        .write(
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  def show_partial(article)\n    \"Helper shows #{article.title}\"\n  end\nend\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h2\", \"Comments\"\n",
            "    assert_select \"h2\", \"Comments\"\n    \
             assert_includes response.body, \"<p id=\\\"show-partial\\\">Helper shows Getting Started with Rails</p>\"\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}
