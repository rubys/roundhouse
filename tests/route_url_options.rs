//! Rails' HOST-ONLY reserved options are stripped from a route-helper
//! call site (`lower::apply_route_url_options_lowering`).
//!
//! `url_for` deletes all of `RouteSet::RESERVED_OPTIONS` from
//! `path_options` before generating a path, so `host:` and its six
//! siblings can never reach the query string. Ours forwarded them, and
//! a leftover option is a query key: campfire's
//! `room_at_message_url(@room, msg, host: "once.campfire.test")`
//! rendered `/rooms/1/@5?host=once.campfire.test`.
//!
//! The demand survey refusing to grow a parameter is only half the fix
//! — the call site has to stop passing one, or the emitted call is an
//! ArgumentError against a helper that has no such keyword. That is the
//! half this suite pins.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_route_url_options_lowering;
use roundhouse::App;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :notes do |t|\n    t.string :body\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    get \"/autocompletable/notes\" => \"notes#index\", :as => \"autocompletable_notes\"\n\
    end\n";

/// Every view body, lowered, as one debug rendering.
fn lowered_views(view: &str) -> String {
    let files = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("config/routes.rb", ROUTES),
        ("app/views/notes/index.html.erb", view),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app: App = ingest_app_from_tree(tree).expect("ingest tree");
    apply_route_url_options_lowering(&mut app);
    app.views.iter().map(|v| format!("{:?}", v.body)).collect::<Vec<_>>().join("\n")
}

/// The wall: campfire's `host:`.
#[test]
fn a_host_option_is_stripped_from_the_call() {
    let out = lowered_views(
        "<%= link_to \"a\", autocompletable_notes_path(host: \"once.campfire.test\") %>\n",
    );
    assert!(
        !out.contains("once.campfire.test"),
        "the host value is gone from the call:\n{out}"
    );
    assert!(
        out.contains("autocompletable_notes_path"),
        "and the helper itself survives:\n{out}"
    );
}

/// All seven, not a special case for `host:`. Each names a piece of
/// the URL's host half, which `path_for` renders not at all.
#[test]
fn every_host_only_option_is_stripped() {
    for (opt, value) in [
        ("host", "\"x.test\""),
        ("protocol", "\"https\""),
        ("port", "3001"),
        ("subdomain", "\"sub\""),
        ("domain", "\"d.test\""),
        ("tld_length", "2"),
        ("only_path", "true"),
    ] {
        let out = lowered_views(&format!(
            "<%= link_to \"a\", autocompletable_notes_path({opt}: {value}) %>\n"
        ));
        assert!(
            !out.contains(&format!("Sym {{ value: Symbol(\"{opt}\") }}")),
            "`{opt}:` must not survive the call:\n{out}"
        );
    }
}

/// A hash that also carried a real query key keeps it — only the
/// reserved names are removed, and the helper keeps the parameter the
/// demand survey built for the rest.
#[test]
fn a_query_key_beside_a_host_option_survives() {
    let out = lowered_views(
        "<%= link_to \"a\", autocompletable_notes_path(room_id: 1, host: \"x.test\") %>\n",
    );
    assert!(!out.contains("x.test"), "host is gone:\n{out}");
    assert!(
        out.contains("Symbol(\"room_id\")"),
        "the query key stays:\n{out}"
    );
}

/// `anchor:` is reserved too, and it is NOT stripped — it is the
/// fragment, it belongs on a path, and `routes_to_library` renders it.
/// Stripping it would silently drop `settings_path(anchor: "external")`
/// down to `/settings`.
#[test]
fn the_anchor_is_left_for_the_helper_to_render() {
    let out = lowered_views(
        "<%= link_to \"a\", autocompletable_notes_path(anchor: \"external\") %>\n",
    );
    assert!(
        out.contains("external"),
        "the anchor is still passed:\n{out}"
    );
}

/// A helper this app's routes do not declare is somebody else's
/// keyword. campfire writes `rails_blob_path(attachment, only_path:
/// true)` — an ActiveStorage helper with its own declared parameter,
/// and stripping the option there would change which overload it means.
#[test]
fn a_non_route_helper_is_untouched() {
    let out = lowered_views(
        "<%= link_to \"a\", rails_blob_path(@note, only_path: true) %>\n",
    );
    assert!(
        out.contains("Symbol(\"only_path\")"),
        "only app routes are rewritten:\n{out}"
    );
}

/// The `_url` spelling is the other half of the rule, and it is not a
/// strip: `host:` IS the host of the URL it asks for.
///
/// campfire's assertion compares a copy-link button — which holds
/// `"http://#{Rails.application.domain}#{…_path}"`, the view lowerer's
/// grounding of a hostless `_url` — against
/// `room_at_message_url(@room, msg, host: "once.campfire.test")`.
/// Dropping the host would leave a bare path on one side of that
/// comparison and an absolute URL on the other.
#[test]
fn a_url_with_a_host_becomes_an_absolute_url() {
    let out = lowered_views(
        "<%= link_to \"a\", autocompletable_notes_url(host: \"once.campfire.test\") %>\n",
    );
    assert!(
        out.contains("StringInterp"),
        "the call is rebuilt as an interpolation:\n{out}"
    );
    assert!(out.contains("once.campfire.test"), "the host survives:\n{out}");
    assert!(out.contains("\"://\""), "as the AUTHORITY, not a query key:\n{out}");
    assert!(
        out.contains("autocompletable_notes_path"),
        "over the generated _path helper:\n{out}"
    );
}

/// `protocol:` rides with the host — bare (`"https"`), the convention
/// `rewrite_url_helpers_absolute` already set for the explicit
/// `…routes.url_helpers` chain.
#[test]
fn a_protocol_option_replaces_the_default_scheme() {
    let out = lowered_views(
        "<%= link_to \"a\", autocompletable_notes_url(host: \"x.test\", protocol: \"https\") %>\n",
    );
    assert!(out.contains("https"), "the protocol survives:\n{out}");
    assert!(
        !out.contains("Text { value: \"http\" }"),
        "and replaces the default rather than joining it:\n{out}"
    );
}

/// `x_url(…, only_path: true)` is Rails asking the URL spelling for a
/// PATH. The strip has already made it one, so no host is prefixed.
#[test]
fn only_path_on_a_url_stays_a_path() {
    let out = lowered_views(
        "<%= link_to \"a\", autocompletable_notes_url(host: \"x.test\", only_path: true) %>\n",
    );
    assert!(!out.contains("x.test"), "no host is rendered:\n{out}");
    assert!(!out.contains("\"://\""), "and no authority either:\n{out}");
}

/// A hostless `_url` is left exactly as it was — the view lowerer
/// grounds it with `Rails.application.domain`, and this pass has
/// nothing to say about a call that named no host.
#[test]
fn a_hostless_url_is_untouched() {
    let out = lowered_views(
        "<%= link_to \"a\", autocompletable_notes_url(room_id: 1) %>\n",
    );
    assert!(!out.contains("\"://\""), "no authority is invented:\n{out}");
    assert!(out.contains("Symbol(\"room_id\")"), "the query key stays:\n{out}");
}
