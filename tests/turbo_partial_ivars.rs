//! The ivars a partial reads thread through every view that renders it,
//! as positional parameters (`view_ivar_closures`). Two ways a caller
//! missed them:
//!
//! - `turbo_stream.<action> target, partial: "x"` renders the partial
//!   through the same call a `render` does, but added no render-graph
//!   edge, so the caller never took the partial's ivars and the emitted
//!   call read an unbound local.
//! - `@notice` and `@alert` read as locals `notice` and `alert`, which
//!   are also every template's flash params. The two collapsed into one
//!   parameter while each caller still passed both, so the call raised
//!   "wrong number of arguments".
//!
//! The analyzer types a partial's ivars from its plain `render` callers
//! only, so each partial a turbo stream renders here is rendered by the
//! index too, which sets the ivar.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const CONTROLLER_TEST: &str = "test/controllers/articles_controller_test.rb";

const HEADING: &str = "  <h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n";

const SHOW_ASSERT: &str = "    assert_select \"h2\", \"Comments\"\n";

const INDEX_HEADING: &str = "    <h1 class=\"font-bold text-4xl\">Articles</h1>\n";

const INDEX_QUERY: &str = "    @articles = Article.includes(:comments).order(created_at: :desc)\n";

/// `overlay` with `show` setting `@<ivar>` to `value`, and the show
/// test asserting `expect` too.
fn show_sets(
    overlay: emit_and_run::Overlay,
    ivar: &str,
    value: &str,
    expect: &str,
) -> emit_and_run::Overlay {
    overlay
        .edit(
            "app/controllers/articles_controller.rb",
            "  def show\n  end\n",
            &format!("  def show\n    @{ivar} = {value:?}\n  end\n"),
        )
        .edit(CONTROLLER_TEST, SHOW_ASSERT, &format!("{SHOW_ASSERT}{expect}"))
}

/// `overlay` with the index rendering `partial` too, and setting
/// `@<ivar>` to `value`.
fn index_renders(
    overlay: emit_and_run::Overlay,
    partial: &str,
    ivar: &str,
    value: &str,
) -> emit_and_run::Overlay {
    overlay
        .edit(
            "app/views/articles/index.html.erb",
            INDEX_HEADING,
            &format!("{INDEX_HEADING}    <%= render \"{partial}\" %>\n"),
        )
        .edit(
            "app/controllers/articles_controller.rb",
            INDEX_QUERY,
            &format!("{INDEX_QUERY}    @{ivar} = {value:?}\n"),
        )
}

const NOTE: &str = "<p id=\"ivar-notice\"><%= @notice %></p>\n";

/// `show` sets `@notice` and the flash notice, each to a value of its
/// own; the template prints the flash, and `expect` is where the ivar
/// shows up.
fn notice_app(expect: &str) -> emit_and_run::Overlay {
    show_sets(
        emit_and_run::real_blog(),
        "notice",
        "Ivar notice",
        &format!(
            "    assert_includes response.body, \"id=\\\"notice\\\">Flash notice</p>\"\n    \
             assert_includes response.body, {expect:?}\n"
        ),
    )
    .edit(
        "app/controllers/articles_controller.rb",
        "    @notice = \"Ivar notice\"\n",
        "    @notice = \"Ivar notice\"\n    flash.now[:notice] = \"Flash notice\"\n",
    )
}

#[test]
fn a_rendered_partial_reading_notice_keeps_the_ivar_apart_from_the_flash() {
    notice_app("<p id=\"ivar-notice\">Ivar notice</p>")
        .write("app/views/articles/_note.html.erb", NOTE)
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!("{HEADING}  <%= render \"note\" %>\n"),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

#[test]
fn a_turbo_stream_partial_reading_notice_keeps_the_ivar_apart_from_the_flash() {
    let app = notice_app("<p id=\"ivar-notice\">Ivar notice</p>")
        .write("app/views/articles/_note.html.erb", NOTE)
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!("{HEADING}  <%= turbo_stream.update \"note\", partial: \"articles/note\" %>\n"),
        );
    index_renders(app, "note", "notice", "Listed notice")
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

#[test]
fn a_layout_reading_notice_keeps_the_ivar_apart_from_the_flash() {
    notice_app("<span id=\"layout-notice\">Ivar notice</span>")
        .edit(
            "app/views/layouts/application.html.erb",
            "      <%= yield %>\n",
            "      <span id=\"layout-notice\"><%= @notice %></span>\n      <%= yield %>\n",
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// `@alert` the same, with the template printing the flash `alert`.
#[test]
fn a_partial_reading_alert_keeps_the_ivar_apart_from_the_flash() {
    show_sets(
        emit_and_run::real_blog(),
        "alert",
        "Ivar alert",
        "    assert_includes response.body, \"<p id=\\\"ivar-alert\\\">Ivar alert</p>\"\n    \
         assert_includes response.body, \"<p id=\\\"flash-alert\\\">Flash alert</p>\"\n",
    )
    .write("app/views/articles/_warning.html.erb", "<p id=\"ivar-alert\"><%= @alert %></p>\n")
    .edit(
        "app/views/articles/show.html.erb",
        HEADING,
        &format!("{HEADING}  <p id=\"flash-alert\"><%= alert %></p>\n  <%= render \"warning\" %>\n"),
    )
    .edit(
        "app/controllers/articles_controller.rb",
        "    @alert = \"Ivar alert\"\n",
        "    @alert = \"Ivar alert\"\n    flash.now[:alert] = \"Flash alert\"\n",
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// `@heading` is read only by the partial, never by `show` itself.
#[test]
fn a_turbo_stream_partial_render_threads_the_partials_ivars() {
    let app = show_sets(
        emit_and_run::real_blog(),
        "heading",
        "Read on",
        "    assert_includes response.body, \"<p id=\\\"heading\\\">Read on</p>\"\n",
    )
    .write("app/views/articles/_heading.html.erb", "<p id=\"heading\"><%= @heading %></p>\n")
    .edit(
        "app/views/articles/show.html.erb",
        HEADING,
        &format!("{HEADING}  <%= turbo_stream.replace \"article-heading\", partial: \"heading\" %>\n"),
    );
    index_renders(app, "heading", "heading", "All of them")
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// The collection form renders the partial once per element, each with
/// its element and the ivar.
#[test]
fn a_turbo_stream_collection_render_threads_the_partials_ivars() {
    show_sets(
        emit_and_run::real_blog(),
        "byline",
        "says hi",
        "    assert_select \"p.byline\", /#{@article.comments.first.commenter} says hi/\n",
    )
    .write(
        "app/views/articles/_byline.html.erb",
        "<p class=\"byline\"><%= comment.commenter %> <%= @byline %></p>\n",
    )
    .edit(
        "app/views/articles/show.html.erb",
        HEADING,
        &format!(
            "{HEADING}  <%= turbo_stream.append \"bylines\", partial: \"articles/byline\", \
             collection: @article.comments, as: :comment %>\n"
        ),
    )
    .edit(
        "app/views/articles/index.html.erb",
        INDEX_HEADING,
        &format!(
            "{INDEX_HEADING}    <%= render partial: \"byline\", collection: Comment.all, as: :comment %>\n"
        ),
    )
    .edit(
        "app/controllers/articles_controller.rb",
        INDEX_QUERY,
        &format!("{INDEX_QUERY}    @byline = \"lists\"\n"),
    )
    .run_test(CONTROLLER_TEST)
    .assert_passes();
}

/// A `.turbo_stream.erb` template the action renders implicitly: the
/// format dispatch passes the ivar the partial reads.
#[test]
fn a_turbo_stream_template_threads_its_partials_ivars() {
    let app = emit_and_run::real_blog()
        .write("app/views/articles/_banner_note.html.erb", "<p id=\"banner\"><%= @banner %></p>\n")
        .write(
            "app/views/articles/banner.turbo_stream.erb",
            "<%= turbo_stream.update \"banner\", partial: \"articles/banner_note\" %>\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  \
             get \"articles/:id/banner\", to: \"articles#banner\", as: :banner_article\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  # GET /articles/new\n",
            "  def banner\n    @banner = \"Streamed\"\n  end\n\n  # GET /articles/new\n",
        )
        .edit(
            CONTROLLER_TEST,
            "  test \"should get edit\" do\n",
            "  test \"the turbo stream carries the banner\" do\n    \
             get banner_article_url(@article, format: :turbo_stream)\n    \
             assert_includes response.body, \"<p id=\\\"banner\\\">Streamed</p>\"\n  end\n\n  \
             test \"should get edit\" do\n",
        );
    index_renders(app, "banner_note", "banner", "Listed")
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// `turbo_stream.append target, @message` with `@message` a String puts
/// the String in the stream and renders no partial, so `messages/_message`
/// (which the index renders, reading `@banner`) adds nothing to the
/// template.
#[test]
fn a_turbo_stream_string_payload_renders_no_partial() {
    let app = show_sets(
        emit_and_run::real_blog(),
        "message",
        "Hello",
        "    assert_includes response.body, \"<template>Hello</template>\"\n",
    )
    .write("app/views/messages/_message.html.erb", "<p><%= @banner %></p>\n")
    .edit(
        "app/views/articles/show.html.erb",
        HEADING,
        &format!("{HEADING}  <%= turbo_stream.append \"messages\", @message %>\n"),
    );
    let run = index_renders(app, "messages/message", "banner", "Listed").run_test(CONTROLLER_TEST);
    run.assert_passes();
    let show = std::fs::read_to_string(run.emitted.join("app/views/articles/show.rb"))
        .expect("show.rb");
    assert!(!show.contains("banner"), "the template takes no `banner`:\n{show}");
}

/// The same for a String `@notice` and `@alert`: each is its own String,
/// whatever the flash holds.
#[test]
fn a_turbo_stream_string_payload_named_like_a_flash_param_renders_no_partial() {
    notice_app("<template>Ivar notice</template>")
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!(
                "{HEADING}  <%= turbo_stream.append \"notices\", @notice %>\n  \
                 <%= turbo_stream.append \"alerts\", @alert %>\n"
            ),
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    @notice = \"Ivar notice\"\n",
            "    @notice = \"Ivar notice\"\n    @alert = \"Ivar alert\"\n",
        )
        .edit(
            CONTROLLER_TEST,
            SHOW_ASSERT,
            &format!(
                "{SHOW_ASSERT}    assert_includes response.body, \"<template>Ivar alert</template>\"\n"
            ),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// A full-template render passes the template its closure: the ivar,
/// not the caller's flash of the same name. (`details` has an action of
/// its own, which is where the analyzer learns `@notice`'s type.)
#[test]
fn a_rendered_template_reading_notice_gets_the_ivar() {
    notice_app("<p id=\"ivar-notice\">Ivar notice</p>")
        .write("app/views/articles/details.html.erb", NOTE)
        .edit(
            "app/controllers/articles_controller.rb",
            "  # GET /articles/new\n",
            "  def details\n    @notice = \"Detail notice\"\n  end\n\n  # GET /articles/new\n",
        )
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!("{HEADING}  <%= render template: \"articles/details\" %>\n"),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// The flash takes a name of its own only where it has to, and not one
/// the template already uses: `notice_` here is the template's own local.
#[test]
fn the_renamed_flash_param_keeps_clear_of_the_templates_locals() {
    notice_app("<p id=\"ivar-notice\">Ivar notice</p><p id=\"own\">Own</p>")
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!(
                "{HEADING}  <% notice_ = \"Own\" %><p id=\"ivar-notice\"><%= @notice %></p>\
                 <p id=\"own\"><%= notice_ %></p>\n"
            ),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// A strict-locals partial declaring a `notice:` local and reading
/// `@notice`: the definition and its callers agree on the arguments.
#[test]
fn a_strict_locals_partial_declaring_notice_renders() {
    notice_app("<p id=\"card\">Local")
        .write(
            "app/views/articles/_card.html.erb",
            "<%# locals: (article:, notice: nil) -%>\n<p id=\"card\"><%= notice %> <%= @notice %></p>\n",
        )
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!("{HEADING}  <%= render \"card\", article: @article, notice: \"Local\" %>\n"),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// The renamed flash keeps clear of the names a view is handed, not only
/// those it spells: a layout's `local_assigns[:notice_]` stays nil.
#[test]
fn the_renamed_flash_param_keeps_clear_of_local_assigns_keys() {
    notice_app("<span id=\"layout-notice\">Ivar notice</span><span id=\"assigned\"></span>")
        .edit(
            "app/views/layouts/application.html.erb",
            "      <%= yield %>\n",
            "      <span id=\"layout-notice\"><%= @notice %></span>\
             <span id=\"assigned\"><%= local_assigns[:notice_] %></span>\n      <%= yield %>\n",
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// … and of the record a collection binds (`as: :notice_`), which the
/// partial takes even though its body never reads it. Def and contracts
/// share that record name, so the flash param is `notice__` on both —
/// not `notice_` on the def and `notice_` on a caller that still thinks
/// the record is `article`.
#[test]
fn the_renamed_flash_param_keeps_clear_of_the_collection_record() {
    let run = notice_app("<p class=\"stamp\">Ivar notice</p>")
        .write("app/views/articles/_stamp.html.erb", "<p class=\"stamp\"><%= @notice %></p>\n")
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!(
                "{HEADING}  <%= render partial: \"stamp\", collection: @article.comments, as: :notice_ %>\n"
            ),
        )
        .run_test(CONTROLLER_TEST);
    run.assert_passes();
    let stamp =
        std::fs::read_to_string(run.emitted.join("app/views/articles/_stamp.rb")).expect("_stamp.rb");
    assert!(
        stamp.contains("notice__") && stamp.contains("def self.stamp(notice_,"),
        "def takes as: record notice_ and flash notice__:\n{stamp}"
    );
}

/// A block parameter named `notice`, here with a default, is a local:
/// its reads are the row, not the flash.
#[test]
fn a_block_parameter_named_notice_is_not_the_flash() {
    notice_app("<p class=\"row\">row</p>")
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!(
                "{HEADING}  <p><%= @notice %></p>\
                 <% [\"row\"].each do |notice = \"fallback\"| %><p class=\"row\"><%= notice %></p><% end %>\n"
            ),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// A partial reading `@notice` whose caller passes it a `notice:` local:
/// the definition and the call agree on the arguments, and the ivar
/// reads as the ivar.
#[test]
fn a_partial_handed_a_notice_local_and_reading_notice_renders() {
    notice_app("<p id=\"handed\">Ivar notice")
        .write("app/views/articles/_handed.html.erb", "<p id=\"handed\"><%= @notice %> / <%= notice %></p>\n")
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!("{HEADING}  <%= render \"handed\", notice: \"Local\" %>\n"),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// A spelling the walker leaves alone (`layout:`) renders no partial, so
/// it adds no edge either: the template takes nothing the partial reads.
#[test]
fn a_turbo_stream_option_the_walker_declines_adds_no_edge() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .write("app/views/articles/_boxed.html.erb", "<p><%= @boxed_title %></p>\n")
        .edit(
            "app/views/articles/show.html.erb",
            HEADING,
            &format!(
                "{HEADING}  <%= turbo_stream.replace \"box\", partial: \"boxed\", layout: \"frame\" %>\n"
            ),
        )
        .emit(BuildTarget::Ruby);
    let show = std::fs::read_to_string(emitted.join("app/views/articles/show.rb")).expect("show.rb");
    assert!(!show.contains("boxed_title"), "the template takes no `boxed_title`:\n{show}");
}
