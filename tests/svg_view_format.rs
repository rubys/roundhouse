//! `render formats: :svg`, and the FOUR gates a new view format passes.
//!
//! campfire renders a user's initials as an SVG avatar
//! (`users/avatars/show.svg.erb`, reached by `render formats: :svg`).
//! Supporting it meant four separate gates, each invisible until the one
//! before it was open:
//!
//!   1. INGEST (`walk_erb`) allowed only `html`/`turbo_stream`, so the
//!      template was never read.
//!   2. EMIT (`renders_through_view_path`) allowed the same two, so once
//!      read it was dropped. A format listed in one and not the other
//!      vanishes silently in between — which is why both are asserted.
//!   3. CONTRACT LOOKUP (`contract_stem`) special-cased `turbo_stream`
//!      only, so it asked for bare `show`. That directory has NO
//!      `show.html.erb`, the lookup missed, and the render became
//!      MissingTemplate for a template sitting right there.
//!   4. THE GUARD: `contains_terminal` sees only a literal render, so an
//!      action whose whole job is picking a private helper looked
//!      terminal-free and got an UNGUARDED tail — raising over the
//!      response its helper had just produced.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\nend\n";

/// No `show.html.erb` — only the svg one, which is campfire's shape and
/// what makes gate 3 bite.
fn app() -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/user.rb"),
        b"class User < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :users\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/users_controller.rb"),
        b"class UsersController < ApplicationController\n  \
            def show\n    @user = User.first\n    render_initials\n  end\n\n  \
            private\n    def render_initials\n      render formats: :svg\n    end\nend\n"
            .to_vec(),
    );
    tree.insert(
        PathBuf::from("app/views/users/show.svg.erb"),
        b"<svg><text><%= @user.name %></text></svg>\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

/// Gates 1 and 2: the template is READ and EMITTED, under a
/// format-qualified name that sits beside `show`.
#[test]
fn an_svg_template_is_ingested_and_emitted() {
    let app = app();
    assert!(
        app.views.iter().any(|v| v.format.as_str() == "svg"),
        "gate 1 — ingest must read it: {:?}",
        app.views.iter().map(|v| (v.name.as_str(), v.format.as_str())).collect::<Vec<_>>()
    );
    let views = ruby::emit_lowered_views(&app)
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        views.contains("def self.show_svg("),
        "gate 2 — emit must keep it, format-qualified:\n{views}"
    );
}

/// Gate 3: `render formats: :svg` binds to that view with the svg MIME —
/// not MissingTemplate, even though no `show.html.erb` exists.
#[test]
fn a_formats_render_binds_the_svg_view() {
    let src = ruby::emit_lowered_controllers_with_layout(&app())
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains("show_svg(") && src.contains(r#"content_type: "image/svg+xml""#),
        "must bind the view and tag the MIME:\n{src}"
    );
    assert!(
        !src.contains("render(formats:"),
        "Rails' `formats:` must not reach the runtime:\n{src}"
    );
}

/// Gate 4: the action's synthesized tail is GUARDED, because the
/// response happens inside a private helper. Unguarded, it raised over
/// the render that had already run.
#[test]
fn a_helper_response_guards_the_synthesized_tail() {
    let src = ruby::emit_lowered_controllers_with_layout(&app())
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    let (_, show_body) = src.split_once("def show").expect("a show action");
    let show_body = show_body.split("def render_initials").next().unwrap_or(show_body);
    assert!(
        show_body.contains("performed?"),
        "the tail must be guarded when a helper responds:\n{show_body}"
    );
}

/// Rails' `default_render`: falling off the end of an action with NO
/// template logs "No template found … rendering head :no_content" and
/// returns 204. We raised MissingTemplate instead, so campfire's
/// `Messages::Boosts#destroy` — a turbo_stream DELETE with no template,
/// asserting `:success` — died on a template Rails never looks for.
///
/// The EXPLICIT `render :foo` path still raises, which is what lobsters'
/// about/privacy actions rescue as their normal flow. That split is the
/// whole point: only the SYNTHESIZED tail changed.
#[test]
fn an_action_with_no_template_heads_no_content() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/user.rb"),
        b"class User < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :users\nend\n".to_vec(),
    );
    // `destroy` has no template anywhere — the shape Rails 204s.
    tree.insert(
        PathBuf::from("app/controllers/users_controller.rb"),
        b"class UsersController < ApplicationController\n  \
            def destroy\n    User.first.destroy!\n  end\nend\n"
            .to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_lowered_controllers_with_layout(&app)
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains("head(:no_content)"),
        "a templateless action must 204, not raise:\n{src}"
    );
    assert!(
        !src.contains("MissingTemplate"),
        "no synthesized raise may survive:\n{src}"
    );
}
