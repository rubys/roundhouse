//! `render plain: body, content_type: "image/svg+xml"`.
//!
//! The content type implied by `plain:` / `html:` / `json:` is a
//! DEFAULT. Rails lets an explicit `content_type:` override it, and the
//! lowering used to append its own entry unconditionally — emitting the
//! key TWICE. A Ruby hash literal is last-wins, so the author's value
//! was written down and then overwritten one entry later: campfire's QR
//! action asked for `image/svg+xml` and shipped `text/plain`, which
//! reads as correct in the emit right up until you check which one Ruby
//! keeps.
//!
//! Asserted on the EMITTED TEXT rather than on the helper, because
//! "which entry survives" is a property of the rendered hash.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn controller_src(action_body: &str) -> String {
    let controller = format!(
        "class ThingsController < ApplicationController\n  def show\n    {action_body}\n  end\nend\n"
    );
    let mut app = ingest_app_from_tree(
        [
            (
                "db/schema.rb",
                "ActiveRecord::Schema.define(version: 1) do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n".to_string(),
            ),
            ("app/models/thing.rb", "class Thing < ApplicationRecord\nend\n".to_string()),
            ("config/routes.rb", "Rails.application.routes.draw do\n  resources :things\nend\n".to_string()),
            ("app/controllers/things_controller.rb", controller),
        ]
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
        .collect::<HashMap<_, _>>(),
    )
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_controllers_with_layout(&app);
    files
        .iter()
        .find(|f| f.path.ends_with("things_controller.rb"))
        .expect("things_controller.rb")
        .content
        .clone()
}

/// The author's `content_type:` stands, and the `plain:` default is not
/// also emitted beside it.
#[test]
fn an_explicit_content_type_survives_the_plain_default() {
    let src = controller_src(r#"render plain: "<svg/>", content_type: "image/svg+xml""#);
    assert!(
        src.contains(r#"content_type: "image/svg+xml""#),
        "the author's content type must survive:\n{src}"
    );
    assert!(
        !src.contains("text/plain"),
        "the plain: default must not be emitted beside it:\n{src}"
    );
    assert_eq!(
        src.matches("content_type:").count(),
        1,
        "exactly one content_type entry:\n{src}"
    );
}

/// With no explicit option, `plain:` still supplies `text/plain` — the
/// fix narrows the append, it does not remove it.
#[test]
fn a_bare_plain_render_still_gets_text_plain() {
    let src = controller_src(r#"render plain: "hello""#);
    assert!(
        src.contains(r#"content_type: "text/plain""#),
        "the plain: default still applies when nothing overrides it:\n{src}"
    );
}
