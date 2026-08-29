//! `X.prepend Y` / `X.include Y` in a `config/initializers/` file.
//!
//! THE ONE INITIALIZER SHAPE THAT CHANGES METHOD LOOKUP. Everything
//! else an initializer does is configuration some lowering reads; this
//! one rewrites an ancestor chain, so dropping it leaves a module
//! defined and unreachable.
//!
//! campfire is the case that forced it. `turbo_streams_authorization.rb`
//! is one line —
//!
//! ```ruby
//! Rails.application.config.to_prepare do
//!   Turbo::StreamsChannel.prepend RoomStreamsAreAuthorized
//! end
//! ```
//!
//! — and it is what makes `RoomMessagesChannel` the only door onto a
//! room's message stream. The concern itself ingests (it lives in
//! `app/channels/concerns/`), so without the prepend the guard is in the
//! tree, tested, and out of the lookup chain: a security control that
//! reads as present and does nothing.
//!
//! Two things are asserted here and they pull opposite ways. A mixin
//! whose constants the tree defines must be EMITTED, at the end of boot
//! where both are loaded. A mixin naming a constant the tree does NOT
//! define must be DROPPED — the line would `NameError` at require time
//! and take the whole boot down — and REPORTED, because silently losing
//! this particular construct is the worse failure of the two.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app_from(initializer: &str, extra: &[(&str, &str)]) -> roundhouse::App {
    let mut files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("config/initializers/mixins.rb", initializer),
    ];
    files.extend_from_slice(extra);
    let mut app = ingest_app_from_tree(tree(&files)).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

/// The concern as campfire files it: a module under
/// `app/channels/concerns/`, which ingests as a library class.
const GUARD: (&str, &str) = (
    "app/channels/concerns/room_streams_are_authorized.rb",
    "module RoomStreamsAreAuthorized\n  def subscribed\n    super\n  end\nend\n",
);

/// A stand-in for the prepend TARGET, so both constants resolve.
const CHANNEL: (&str, &str) = (
    "app/channels/streams_channel.rb",
    "class StreamsChannel\n  def subscribed\n    nil\n  end\nend\n",
);

#[test]
fn a_top_level_prepend_is_recorded() {
    let app = app_from(
        "StreamsChannel.prepend RoomStreamsAreAuthorized\n",
        &[GUARD, CHANNEL],
    );
    assert_eq!(app.module_mixins.len(), 1, "{:?}", app.module_mixins);
    let m = &app.module_mixins[0];
    assert_eq!(m.target.as_str(), "StreamsChannel");
    assert_eq!(m.module.as_str(), "RoomStreamsAreAuthorized");
    assert_eq!(m.kind, roundhouse::app::MixinKind::Prepend);
}

/// campfire's own spelling. The block is unwrapped rather than modeled:
/// to a tree that boots once, `to_prepare` means exactly its body.
#[test]
fn a_to_prepare_block_is_unwrapped() {
    let app = app_from(
        "Rails.application.config.to_prepare do\n  \
         StreamsChannel.prepend RoomStreamsAreAuthorized\nend\n",
        &[GUARD, CHANNEL],
    );
    assert_eq!(app.module_mixins.len(), 1, "{:?}", app.module_mixins);
    assert_eq!(app.module_mixins[0].target.as_str(), "StreamsChannel");
}

#[test]
fn include_is_recorded_distinctly_from_prepend() {
    let app = app_from("StreamsChannel.include RoomStreamsAreAuthorized\n", &[GUARD, CHANNEL]);
    assert_eq!(app.module_mixins.len(), 1);
    assert_eq!(app.module_mixins[0].kind, roundhouse::app::MixinKind::Include);
}

/// The emitted line lands at the END of boot.rb, after `app/models` and
/// `app/views`: a mixin names two constants and both have to be loaded.
///
/// Emitted from the real-blog fixture — `target_files` reads a fixture
/// from disk, so the in-memory trees above cannot reach the emit half.
/// The mixin is pushed onto the App directly, which is exactly the state
/// `lower::module_mixins` leaves behind for one it kept.
#[test]
fn a_resolvable_mixin_reaches_the_end_of_boot() {
    use roundhouse::app::{MixinKind, ModuleMixin};
    use roundhouse::ident::Symbol;
    use roundhouse::project::{target_files, BuildTarget};

    let fixture = PathBuf::from("fixtures/real-blog");
    let mut app = roundhouse::ingest::ingest_app(&fixture).expect("ingest real-blog");
    roundhouse::session::analyze_and_lower(&mut app);
    app.module_mixins.push(ModuleMixin {
        target: Symbol::from("Article"),
        module: Symbol::from("Publishable"),
        kind: MixinKind::Prepend,
    });

    let files = target_files(&app, &fixture, BuildTarget::Ruby).expect("ruby target files");
    let boot = files
        .iter()
        .find(|(p, _)| p == "boot.rb")
        .map(|(_, c)| c.clone())
        .expect("boot.rb");

    assert!(
        boot.contains("Article.prepend Publishable"),
        "the prepend never reached boot.rb:\n{boot}"
    );
    let mixin_at = boot.find("Article.prepend Publishable").unwrap();
    let models_at = boot.find("require_relative \"app/models\"").unwrap();
    assert!(
        mixin_at > models_at,
        "the mixin must run AFTER app/models loads, or neither constant is defined"
    );
}

/// The gap that exists today: campfire prepends onto
/// `Turbo::StreamsChannel`, which no tree defines. Emitting that line
/// would `NameError` at require time and take the boot down, so it is
/// dropped — and reported, because a silently-vanished authorization
/// prepend reads as a working guard.
#[test]
fn a_mixin_naming_an_undefined_constant_is_dropped_and_reported() {
    let mut app = {
        let files = vec![
            ("db/schema.rb", SCHEMA),
            ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
            (
                "config/initializers/mixins.rb",
                "Rails.application.config.to_prepare do\n  \
                 Turbo::StreamsChannel.prepend RoomStreamsAreAuthorized\nend\n",
            ),
            GUARD,
        ];
        ingest_app_from_tree(tree(&files)).expect("ingest")
    };
    // Recorded at ingest — resolution is not ingest's question.
    assert_eq!(app.module_mixins.len(), 1);

    let diags = roundhouse::session::analyze_and_lower(&mut app);

    assert!(app.module_mixins.is_empty(), "an unresolvable mixin must not be emitted");

    let reported = diags.iter().any(|d| {
        d.message.contains("Turbo::StreamsChannel") && d.message.contains("not defined in this tree")
    });
    assert!(
        reported,
        "dropping it silently leaves the guard defined and out of the lookup chain; \
         diagnostics were:\n{:#?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// An app with no such initializer carries no mixins at all.
#[test]
fn an_app_without_mixins_is_untouched() {
    let app = app_from("# nothing here\n", &[]);
    assert!(app.module_mixins.is_empty());
}
