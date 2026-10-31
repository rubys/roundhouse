//! A partial renders in its caller's view context, so a bare `notice` or
//! `alert` in it is the request's flash, as it is in the template that
//! renders it. Every template takes the flash pair as trailing params
//! (`collect_extra_params`); a render passed them only from an explicit
//! `locals:` hash, so a partial's `notice` was nil wherever it was
//! rendered from — a view, a layout, a collection, another partial, or
//! an action's `render partial:`. The flash is only ever the flash: a
//! `local_assigns[:notice_]` is no flash slot, a `local_assigns[:notice]`
//! is the render's `notice:` local or nil, and a partial's `notice:`
//! local does not reach the partials it renders.
//!
//! Each test follows the blog's own redirects, so the flash rides the
//! cookie to the page the partial is on.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CONTROLLER_TEST: &str = "test/controllers/articles_controller_test.rb";

const HEADING: &str = "  <h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n";

const CREATE_ASSERT: &str = "    assert_equal \"New Title\", Article.last.title\n";

const NOTE: &str = "<p id=\"partial-notice\"><%= notice %></p>\n";

const CREATED: &str = "<p id=\"partial-notice\">Article was successfully created.</p>";

const OPT: &str = "app/views/articles/_flash_opt.html.erb";

const OPT_ERB: &str = "<p id=\"opt\"><%= notice %>|<%= local_assigns[:notice_] %></p>\n";

const LOCAL: &str = "app/views/articles/_flash_local.html.erb";

const LOCAL_ERB: &str = "<p class=\"local\"><%= notice %>|<%= local_assigns[:notice] || \"none\" %></p>\n";

/// The blog with `show` rendering `erb` under its heading, and the create
/// test following the redirect to it and asserting each of `expect`.
fn shown_after_create(erb: &str, expect: &[&str]) -> emit_and_run::Overlay {
    let asserts: String = expect
        .iter()
        .map(|e| format!("    assert_includes response.body, {e:?}\n"))
        .collect();
    emit_and_run::real_blog()
        .write("app/views/articles/_flash_note.html.erb", NOTE)
        .edit("app/views/articles/show.html.erb", HEADING, &format!("{HEADING}{erb}"))
        .edit(
            CONTROLLER_TEST,
            CREATE_ASSERT,
            &format!("{CREATE_ASSERT}    follow_redirect!\n{asserts}"),
        )
}

#[test]
fn a_partial_a_view_renders_reads_the_flash() {
    shown_after_create("  <%= render \"flash_note\" %>\n", &[CREATED])
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

#[test]
fn a_partial_the_layout_renders_reads_the_flash() {
    shown_after_create("", &["<p id=\"layout-notice\">Article was successfully created.</p>"])
        .write("app/views/shared/_flash.html.erb", "<p id=\"layout-notice\"><%= notice %></p>\n")
        .edit(
            "app/views/layouts/application.html.erb",
            "      <%= yield %>\n",
            "      <%= render \"shared/flash\" %>\n      <%= yield %>\n",
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// The `alert` slot too, from a partial with a record and `locals:` that
/// name neither flash slot.
#[test]
fn a_partial_handed_other_locals_reads_the_alert() {
    shown_after_create(
        "  <%= render partial: \"flash_alert\", locals: { article: @article } %>\n",
        &["<p id=\"partial-alert\">New Title: Check it</p>"],
    )
    .write(
        "app/views/articles/_flash_alert.html.erb",
        "<p id=\"partial-alert\"><%= article.title %>: <%= alert %></p>\n",
    )
    .edit(
        "app/controllers/articles_controller.rb",
        "redirect_to @article, notice: \"Article was successfully created.\"",
        "redirect_to @article, notice: \"Article was successfully created.\", alert: \"Check it\"",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

#[test]
fn each_element_of_a_named_collection_reads_the_flash() {
    shown_after_create(
        "  <%= render partial: \"flash_row\", collection: Article.where(title: \"New Title\"), as: :article %>\n",
        &["<p class=\"flash-row\">New Title: Article was successfully created.</p>"],
    )
    .write(
        "app/views/articles/_flash_row.html.erb",
        "<p class=\"flash-row\"><%= article.title %>: <%= notice %></p>\n",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// `render @articles` on the index the destroy redirects to.
#[test]
fn each_record_of_a_collection_render_reads_the_flash() {
    emit_and_run::real_blog()
        .edit(
            "app/views/articles/_article.html.erb",
            "<div id=\"<%= dom_id(article) %>\"",
            "<p class=\"article-notice\"><%= notice %></p>\n<div id=\"<%= dom_id(article) %>\"",
        )
        .edit(
            CONTROLLER_TEST,
            "    assert_redirected_to articles_url\n",
            "    assert_redirected_to articles_url\n    follow_redirect!\n    \
             assert_includes response.body, \"<p class=\\\"article-notice\\\">Article was successfully destroyed.</p>\"\n",
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

#[test]
fn a_partial_another_partial_renders_reads_the_flash() {
    shown_after_create("  <%= render \"flash_outer\" %>\n", &["<div id=\"outer\">", CREATED])
        .write(
            "app/views/articles/_flash_outer.html.erb",
            "<div id=\"outer\"><%= render \"flash_note\" %></div>\n",
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// A caller reading `@notice` takes its flash as `notice_`
/// (`flash_locals`); the partial still gets the flash, not the ivar.
#[test]
fn a_caller_with_a_notice_ivar_passes_its_flash() {
    shown_after_create(
        "  <p id=\"ivar-notice\"><%= @notice %></p>\n  <%= render \"flash_note\" %>\n",
        &["<p id=\"ivar-notice\">Ivar notice</p>", CREATED],
    )
    .edit(
        "app/controllers/articles_controller.rb",
        "  def show\n  end\n",
        "  def show\n    @notice = \"Ivar notice\"\n  end\n",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// An explicit `notice:` local is what that render reads; a render of
/// the same partial without one still reads the flash.
#[test]
fn an_explicit_notice_local_wins_over_the_flash() {
    shown_after_create(
        "  <%= render partial: \"flash_note\", locals: { notice: \"Local\" } %>\n  <%= render \"flash_note\" %>\n",
        &["<p id=\"partial-notice\">Local</p>", CREATED],
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// `local_assigns[:notice_]` is an optional local like any other, though
/// its name is how `flash_locals` spells a renamed flash: a render that
/// omits it leaves it nil, beside the partial's flash.
#[test]
fn an_omitted_notice_underscore_local_stays_nil() {
    shown_after_create(
        "  <%= render \"flash_opt\" %>\n",
        &["<p id=\"opt\">Article was successfully created.|</p>"],
    )
    .write(OPT, OPT_ERB)
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// `local_assigns[:notice]` is the render's `notice:` local, nil when it
/// passes none, never the flash a bare `notice` beside it falls back to.
/// A `notice: nil` local is nil for both.
#[test]
fn local_assigns_notice_is_only_the_local() {
    shown_after_create(
        "  <%= render \"flash_local\" %>\n  <%= render \"flash_local\", notice: \"Local\" %>\n  \
         <%= render \"flash_local\", notice: nil %>\n",
        &[
            "<p class=\"local\">Article was successfully created.|none</p>",
            "<p class=\"local\">Local|Local</p>",
            "<p class=\"local\">|none</p>",
        ],
    )
    .write(LOCAL, LOCAL_ERB)
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// The same for `local_assigns[:alert]`, per element of a collection.
#[test]
fn local_assigns_alert_is_only_the_local_per_element() {
    shown_after_create(
        "  <%= render partial: \"flash_alert_row\", collection: Article.where(title: \"New Title\"), as: :article %>\n",
        &["<p class=\"alert-row\">New Title: Check it|none</p>"],
    )
    .write(
        "app/views/articles/_flash_alert_row.html.erb",
        "<p class=\"alert-row\"><%= article.title %>: <%= alert %>|<%= local_assigns[:alert] || \"none\" %></p>\n",
    )
    .edit(
        "app/controllers/articles_controller.rb",
        "redirect_to @article, notice: \"Article was successfully created.\"",
        "redirect_to @article, notice: \"Article was successfully created.\", alert: \"Check it\"",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// A partial handing its own `local_assigns[:notice]` on hands the local
/// or nil, never its flash.
#[test]
fn a_partial_hands_on_its_local_assigns_notice() {
    shown_after_create(
        "  <%= render \"flash_pass\" %>\n  <%= render \"flash_pass\", notice: \"Local\" %>\n",
        &[
            "<div class=\"pass\"><p class=\"local\">|none</p>",
            "<div class=\"pass\"><p class=\"local\">Local|Local</p>",
        ],
    )
    .write(LOCAL, LOCAL_ERB)
    .write(
        "app/views/articles/_flash_pass.html.erb",
        "<div class=\"pass\"><%= render \"flash_local\", notice: local_assigns[:notice] %></div>\n",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// An action's template has no locals, so its `local_assigns[:notice]`
/// is nil beside the flash.
#[test]
fn a_template_s_local_assigns_notice_is_nil() {
    shown_after_create(
        "  <p id=\"template-local\"><%= local_assigns[:notice] || \"none\" %>|<%= notice %></p>\n",
        &["<p id=\"template-local\">none|Article was successfully created.</p>"],
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// Rails does not hand a partial's locals on: the partial it renders
/// reads its own `notice`, never the outer one's `notice:` local.
#[test]
fn an_outer_partial_s_notice_local_is_not_the_inner_flash() {
    shown_after_create(
        "  <%= render partial: \"flash_local_outer\", locals: { notice: \"Local\" } %>\n",
        &["<div id=\"local-outer\">Local<p id=\"partial-notice\">"],
    )
    .write(
        "app/views/articles/_flash_local_outer.html.erb",
        "<div id=\"local-outer\"><%= notice %><%= render \"flash_note\" %></div>\n",
    )
    .edit(
        CONTROLLER_TEST,
        "    follow_redirect!\n",
        "    follow_redirect!\n    \
         refute_includes response.body, \"<p id=\\\"partial-notice\\\">Local</p>\"\n",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// The same for a strict-locals partial's declared `notice:`.
#[test]
fn a_strict_locals_caller_s_notice_is_not_the_inner_flash() {
    shown_after_create(
        "  <%= render \"flash_strict_outer\", article: @article %>\n",
        &["<div id=\"strict-outer\">Declared<p id=\"partial-notice\">"],
    )
    .write(
        "app/views/articles/_flash_strict_outer.html.erb",
        "<%# locals: (article:, notice: \"Declared\") -%>\n\
         <div id=\"strict-outer\"><%= notice %><%= render \"flash_note\" %></div>\n",
    )
    .edit(
        CONTROLLER_TEST,
        "    follow_redirect!\n",
        "    follow_redirect!\n    \
         refute_includes response.body, \"<p id=\\\"partial-notice\\\">Declared</p>\"\n",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// A block param named `notice` or `alert` is a local of the caller's,
/// not the flash a partial rendered in the block reads; a render outside
/// the block still passes the flash.
#[test]
fn a_block_param_named_as_the_flash_is_not_the_inner_flash() {
    shown_after_create(
        "  <% [\"Local\"].each do |notice| %><%= render \"flash_pair\" %><% end %>\n  \
         <% [\"Local\"].each do |alert| %><%= render \"flash_pair\" %><% end %>\n  \
         <%= render \"flash_pair\" %>\n",
        &["<p class=\"pair\">Article was successfully created.|Check it</p>"],
    )
    .write(
        "app/views/articles/_flash_pair.html.erb",
        "<p class=\"pair\"><%= notice %>|<%= alert %></p>\n",
    )
    .edit(
        "app/controllers/articles_controller.rb",
        "redirect_to @article, notice: \"Article was successfully created.\"",
        "redirect_to @article, notice: \"Article was successfully created.\", alert: \"Check it\"",
    )
    .edit(
        CONTROLLER_TEST,
        "    follow_redirect!\n",
        "    follow_redirect!\n    \
         refute_includes response.body, \"<p class=\\\"pair\\\">Local|\"\n    \
         refute_includes response.body, \"|Local</p>\"\n",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// The same for a local assigned over `notice`, from the assignment on:
/// a render before it still passes the flash.
#[test]
fn an_assigned_notice_local_is_not_the_inner_flash() {
    shown_after_create(
        "  <%= render \"flash_note\" %>\n  <% notice = \"Local\" %>\n  <%= render \"flash_note\" %>\n",
        &[CREATED],
    )
    .edit(
        CONTROLLER_TEST,
        "    follow_redirect!\n",
        "    follow_redirect!\n    \
         refute_includes response.body, \"<p id=\\\"partial-notice\\\">Local</p>\"\n",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// A strict-locals partial that declares `notice:` reads its own local,
/// the header's nil, as Rails does — singly and per collection element.
#[test]
fn a_strict_locals_partial_keeps_its_declared_notice() {
    shown_after_create(
        "  <%= render \"flash_card\", article: @article %>\n  \
         <%= render partial: \"flash_card\", collection: Article.where(title: \"New Title\"), as: :article %>\n",
        &[],
    )
    .write(
        "app/views/articles/_flash_card.html.erb",
        "<%# locals: (article:, notice: nil) -%>\n<p class=\"card\"><%= article.title %>:<%= notice %></p>\n",
    )
    .edit(
        CONTROLLER_TEST,
        "    follow_redirect!\n",
        "    follow_redirect!\n    \
         assert_equal 2, response.body.scan(\"<p class=\\\"card\\\">New Title:</p>\").size\n",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// An action's `render partial:` renders in the request's view context
/// too, so the partial reads the action's `flash.now`.
#[test]
fn a_partial_an_action_renders_reads_the_flash() {
    rendered_by_an_action(
        "flash_note",
        NOTE,
        "<p id=\"partial-notice\">Rendered now</p>",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// And its `local_assigns[:notice_]` stays nil there too.
#[test]
fn an_action_s_partial_leaves_an_omitted_notice_underscore_local_nil() {
    rendered_by_an_action("flash_opt", OPT_ERB, "<p id=\"opt\">Rendered now|</p>")
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// As is its `local_assigns[:notice]`, and an action's `notice:` local
/// is what both read.
#[test]
fn an_action_s_partial_reads_local_assigns_notice_as_the_local() {
    rendered_by_an_action("flash_local", LOCAL_ERB, "<p class=\"local\">Rendered now|none</p>")
        .run_test(CONTROLLER_TEST)
        .assert_passes();
    rendered_by_an_action_with(
        "flash_local",
        LOCAL_ERB,
        ", locals: { notice: \"Given\" }",
        "<p class=\"local\">Given|Given</p>",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// The blog with an action that sets `flash.now[:notice]` and renders
/// the partial `name` (`erb`), and a test asserting `expect` in it.
fn rendered_by_an_action(name: &str, erb: &str, expect: &str) -> emit_and_run::Overlay {
    rendered_by_an_action_with(name, erb, "", expect)
}

/// `rendered_by_an_action` with `options` after the render's partial name.
fn rendered_by_an_action_with(name: &str, erb: &str, options: &str, expect: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(&format!("app/views/articles/_{name}.html.erb"), erb)
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            &format!(
                "  root \"articles#index\"\n  \
                 get \"articles/:id/{name}\", to: \"articles#{name}\", as: :{name}_article\n"
            ),
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  # GET /articles/new\n",
            &format!(
                "  def {name}\n    flash.now[:notice] = \"Rendered now\"\n    \
                 render partial: \"{name}\"{options}\n  end\n\n  # GET /articles/new\n"
            ),
        )
        .edit(
            CONTROLLER_TEST,
            "  test \"should get edit\" do\n",
            &format!(
                "  test \"the action's partial reads the flash\" do\n    \
                 get {name}_article_url(@article)\n    \
                 assert_includes response.body, {expect:?}\n  end\n\n  \
                 test \"should get edit\" do\n"
            ),
        )
}
