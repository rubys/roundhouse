//! An app's binary files reach the emitted tree.
//!
//! Every `EmittedFile`'s content is a `String`, so a JPEG, a font or a
//! binary test fixture could not be represented at all — `collect_asset_files`
//! says "skip binary / non-UTF-8" and the main emit never even looked.
//! The result was silent: campfire emitted 900+ files and not one of the
//! images its own views serve or its own tests open, so
//! `file_fixture("pixel.bmp").open` had nothing to open and
//! `send_file Rails.root.join("app/assets/images/logos/app-icon.png")`
//! had nothing to send.
//!
//! These are copied VERBATIM — there is nothing in a JPEG to transpile —
//! which is why they ride a channel of their own rather than becoming
//! `EmittedFile`s with a widened content type.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :things do |t|\n    t.string :name\n  end\nend\n";

/// Two bytes no UTF-8 decoder accepts: a lone continuation byte and an
/// unfinished 2-byte lead. Enough to make `read_to_string` fail, which
/// is the actual predicate the walk uses.
const NOT_UTF8: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x80, 0xC3];

fn app_with(extra: Vec<(&str, Vec<u8>)>) -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/thing.rb"),
        b"class Thing < ApplicationRecord\nend\n".to_vec(),
    );
    for (path, bytes) in extra {
        tree.insert(PathBuf::from(path), bytes);
    }
    ingest_app_from_tree(tree).expect("ingest")
}

fn paths(app: &roundhouse::App) -> Vec<String> {
    app.binary_assets.iter().map(|(p, _)| p.clone()).collect()
}

/// A binary test fixture is carried, with its bytes intact.
#[test]
fn a_binary_test_fixture_is_carried_verbatim() {
    let app = app_with(vec![("test/fixtures/files/pixel.bmp", NOT_UTF8.to_vec())]);
    assert_eq!(paths(&app), vec!["test/fixtures/files/pixel.bmp".to_string()]);
    assert_eq!(
        app.binary_assets[0].1, NOT_UTF8,
        "bytes must survive unchanged — a copy is the whole point"
    );
}

/// Images under `app/assets` and files under `public` ride the same
/// walk; all three roots are covered.
#[test]
fn every_asset_root_is_walked() {
    let app = app_with(vec![
        ("app/assets/images/logos/app-icon.png", NOT_UTF8.to_vec()),
        ("public/favicon.ico", NOT_UTF8.to_vec()),
        ("test/fixtures/files/moon.jpg", NOT_UTF8.to_vec()),
    ]);
    assert_eq!(
        paths(&app),
        vec![
            "app/assets/images/logos/app-icon.png".to_string(),
            "public/favicon.ico".to_string(),
            "test/fixtures/files/moon.jpg".to_string(),
        ],
        "sorted, so a byte-compared emit is stable across runs"
    );
}

/// A UTF-8 file under the same roots is NOT collected — the emitters
/// already carry text, and a second copy here would fight them.
#[test]
fn a_text_asset_under_the_same_root_is_left_to_the_emitters() {
    let app = app_with(vec![
        ("app/assets/tailwind.css", b"body { color: red }\n".to_vec()),
        ("public/robots.txt", b"User-agent: *\n".to_vec()),
    ]);
    assert!(
        paths(&app).is_empty(),
        "text is representable as an EmittedFile: {:?}",
        paths(&app)
    );
}

/// A binary file OUTSIDE the named roots is not swept up. A Rails
/// checkout also holds `node_modules` and `.git`; walking those would
/// cost more than the rest of ingest and ship nothing an app needs.
#[test]
fn binaries_outside_the_named_roots_are_not_collected() {
    let app = app_with(vec![
        ("node_modules/pkg/logo.png", NOT_UTF8.to_vec()),
        ("vendor/cache/gem.gem", NOT_UTF8.to_vec()),
    ]);
    assert!(paths(&app).is_empty(), "{:?}", paths(&app));
}

/// An app with no binaries at all gets an empty list, not a surprise.
#[test]
fn an_app_with_no_binaries_collects_nothing() {
    assert!(paths(&app_with(vec![])).is_empty());
}
