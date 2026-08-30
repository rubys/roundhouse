//! Route-helper options that name no path SEGMENT become the query
//! string — `autocompletable_users_path(room_id: room.id)` →
//! `/autocompletable/users?room_id=1`.
//!
//! Rails turns every non-segment option into a query param at call
//! time. A generated helper has a fixed signature, so the keys have to
//! appear in it, and they arrive as optional KEYWORD params: the call
//! sites already written bind unchanged and every other caller is
//! unaffected.
//!
//! Demand-gated — a helper nobody passes options to keeps exactly the
//! signature it had.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_routes_to_library_functions;
use roundhouse::ty::{ParamKind, Ty};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :rooms do |t|\n    t.string :name\n  end\n  \
    create_table :notes do |t|\n    t.string :body\n  end\nend\n";

fn app_with(routes: &str, view: &str) -> roundhouse::App {
    let files = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("config/routes.rb", routes),
        ("app/views/rooms/index.html.erb", view),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

fn params(app: &roundhouse::App, name: &str) -> Vec<roundhouse::ty::Param> {
    let helpers = lower_routes_to_library_functions(app);
    let sig = helpers
        .iter()
        .find(|f| f.name.as_str() == name)
        .unwrap_or_else(|| {
            panic!(
                "helper {name} not generated; got: {:?}",
                helpers.iter().map(|f| f.name.as_str().to_string()).collect::<Vec<_>>()
            )
        })
        .signature
        .clone()
        .expect("helper signature");
    let Ty::Fn { params, .. } = sig else { panic!("not a Ty::Fn: {sig:?}") };
    params
}

/// The `#` literal `append_anchor` emits, as it reads in the debug
/// rendering of the body — a bare `"#"` would also match the `#{}` of
/// every interpolation, so match the InterpPart it actually is.
const FRAGMENT: &str = "Text { value: \"#\" }";

/// The generated helper's body, as the debug rendering the sibling
/// `direct_url_helpers` suite reads.
fn body(app: &roundhouse::App, name: &str) -> String {
    let helpers = lower_routes_to_library_functions(app);
    let f = helpers
        .iter()
        .find(|f| f.name.as_str() == name)
        .unwrap_or_else(|| panic!("helper {name} not generated"));
    format!("{:?}", f.body)
}

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    get \"/autocompletable/notes\" => \"notes#index\", :as => \"autocompletable_notes\"\n  \
    get \"/rooms/:room_id/involvement\" => \"rooms#involvement\", :as => \"room_involvement\"\n  \
    get \"/mod/notes(/:period)\" => \"notes#index\", :as => \"mod_notes\"\nend\n";

/// The wall this exists for: a helper with no segments at all, called
/// with one option.
#[test]
fn a_non_segment_option_becomes_an_optional_keyword_param() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", autocompletable_notes_path(room_id: 1) %>\n",
    );
    let ps = params(&app, "autocompletable_notes_path");
    assert_eq!(ps.len(), 1, "exactly the one demanded key: {ps:?}");
    assert_eq!(ps[0].name.as_str(), "room_id");
    assert!(
        matches!(ps[0].kind, ParamKind::Keyword { required: false }),
        "an optional KEYWORD, so the written call site binds unchanged \
         — a positional here is a call spinel cannot make: {:?}",
        ps[0].kind
    );
}

/// A helper nobody passes options to is untouched. This is what keeps
/// every existing call site — and every corpus app's emit — the same.
#[test]
fn an_unused_helper_keeps_its_signature() {
    let app = app_with(ROUTES, "<%= link_to \"a\", autocompletable_notes_path %>\n");
    assert!(
        params(&app, "autocompletable_notes_path").is_empty(),
        "no demand, no parameter"
    );
}

/// An option naming one of the route's own segments is NOT a query
/// param. `flatten_routes` expands `(/:period)` into two same-named
/// routes — one carrying `period`, one not — so the segment set has to
/// UNION across them. Reading only one produced `mod_notes_path(period
/// = nil, period: nil)`, a duplicate parameter name.
#[test]
fn a_segment_named_option_is_not_a_query_param() {
    let app = app_with(ROUTES, "<%= link_to \"a\", mod_notes_path(period: \"2w\") %>\n");
    let ps = params(&app, "mod_notes_path");
    assert_eq!(
        ps.iter().filter(|p| p.name.as_str() == "period").count(),
        1,
        "`period` appears once, as the segment it is: {ps:?}"
    );
    assert!(
        !matches!(ps[0].kind, ParamKind::Keyword { .. }),
        "and stays the positional segment: {:?}",
        ps[0].kind
    );
}

/// Segments and query keys coexist — campfire's `room_involvement_path(
/// room_id, involvement: …)`.
#[test]
fn segments_keep_their_positions_ahead_of_the_query_keys() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", room_involvement_path(1, involvement: \"all\") %>\n",
    );
    let ps = params(&app, "room_involvement_path");
    let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["room_id", "involvement"], "order: {ps:?}");
    assert!(matches!(ps[0].kind, ParamKind::Required));
    assert!(matches!(ps[1].kind, ParamKind::Keyword { required: false }));
}

/// A call that does not fill the helper's REQUIRED segments is not a
/// query call. lobsters writes `user_path(:user => name)` against
/// `/u/:username` — invalid Rails either way, and adding a `user:`
/// keyword would only change which error it raises on a page that
/// renders today.
#[test]
fn a_call_missing_its_required_segments_is_left_alone() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", room_involvement_path(involvement: \"all\") %>\n",
    );
    let ps = params(&app, "room_involvement_path");
    assert_eq!(
        ps.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        vec!["room_id"],
        "no query key harvested from a malformed call: {ps:?}"
    );
}

/// `format:` is not a QUERY key and not a PARAMETER either: it is a
/// trailing `.ext` the `route_format_suffix` lowering moves to the call
/// site (a parameter would widen the signature for every caller, which
/// Rust and Go — no default arguments — charge in full).
#[test]
fn format_takes_no_parameter() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", autocompletable_notes_path(format: :json) %>\n",
    );
    let ps = params(&app, "autocompletable_notes_path");
    assert!(ps.is_empty(), "format becomes no parameter: {ps:?}");
}

/// Rails' HOST-ONLY reserved options describe the half of a URL a
/// `_path` helper does not render, so none of them is a query key —
/// `room_at_message_path(1, 5, host: "x")` is `/rooms/1/@5`, not
/// `/rooms/1/@5?host=x`.
///
/// The wall this closes: campfire's own broadcast assertion compares
/// the URL a copy-link button holds against `room_at_message_url(@room,
/// Message.last, host: "once.campfire.test")`, and the trailing
/// `?host=once.campfire.test` was the whole difference.
#[test]
fn host_only_options_are_not_query_keys() {
    for opt in [
        "host: \"x\"",
        "protocol: \"https\"",
        "port: 3000",
        "subdomain: \"a\"",
        "domain: \"b.test\"",
        "tld_length: 2",
        "only_path: true",
    ] {
        let app = app_with(
            ROUTES,
            &format!("<%= link_to \"a\", autocompletable_notes_path({opt}) %>\n"),
        );
        let ps = params(&app, "autocompletable_notes_path");
        assert!(ps.is_empty(), "`{opt}` must grow no parameter: {ps:?}");
    }
}

/// `anchor:` is the one reserved option that DOES belong on a path, so
/// unlike the host-only seven it is harvested — as a keyword parameter
/// rendering `#tag`.
///
/// It previously grew no parameter while four lobsters call sites and
/// two writebook ones kept passing one: `RouteHelpers.settings_path(
/// anchor: "external")` against `def self.settings_path`, an
/// ArgumentError on every page linking to `settings#external`.
#[test]
fn anchor_takes_a_parameter_and_renders_a_fragment() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", autocompletable_notes_path(anchor: \"x\") %>\n",
    );
    let ps = params(&app, "autocompletable_notes_path");
    assert_eq!(
        ps.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        vec!["anchor"],
        "anchor is a parameter: {ps:?}"
    );
    assert!(
        matches!(ps[0].kind, ParamKind::Keyword { required: false }),
        "an optional keyword, like every other option: {:?}",
        ps[0].kind
    );
    let src = body(&app, "autocompletable_notes_path");
    assert!(src.contains(FRAGMENT), "the body renders a `#` fragment:\n{src}");
}

/// `path_for` runs `add_params` and THEN `add_anchor`, so the fragment
/// trails the whole query string. An anchor rendered first would put
/// the `?room_id=1` INSIDE the fragment.
#[test]
fn the_anchor_renders_after_the_query_string() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", autocompletable_notes_path(room_id: 1, anchor: \"x\") %>\n",
    );
    let ps = params(&app, "autocompletable_notes_path");
    assert_eq!(
        ps.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        vec!["room_id", "anchor"],
        "anchor is ordered last: {ps:?}"
    );
    let src = body(&app, "autocompletable_notes_path");
    let q = src.find("?room_id=").unwrap_or_else(|| panic!("no query key:\n{src}"));
    let a = src.find(FRAGMENT).unwrap_or_else(|| panic!("no fragment:\n{src}"));
    assert!(q < a, "the query string is built before the fragment:\n{src}");
}

/// An Array value renders as `k[]=a&k[]=b` in Rails (campfire writes
/// `rooms_directs_path(user_ids: [ user.id ])`), so the parameter it
/// demands is an ARRAY — a scalar parameter would render the array's
/// `to_s` into the URL, and a wrong URL is worse than a missing helper.
///
/// This test previously pinned the opposite: arrays were declined
/// outright, which left the call site passing an argument to a 0-arg
/// helper (`wrong number of arguments (given 1, expected 0)`, 10 tests
/// of campfire's suite). Declining was the honest placeholder; typing
/// it is the implementation.
#[test]
fn an_array_valued_option_takes_an_array_parameter() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", autocompletable_notes_path(user_ids: [ 1 ]) %>\n",
    );
    let ps = params(&app, "autocompletable_notes_path");
    let user_ids = ps
        .iter()
        .find(|p| p.name.as_str() == "user_ids")
        .unwrap_or_else(|| panic!("no user_ids parameter: {ps:?}"));
    // `Array[Integer] | Nil` — nilable because every query key is a
    // keyword defaulting to nil, and `Integer` because `param_ty`
    // types the SINGULAR (`user_id`) it is named after.
    let Ty::Union { variants } = &user_ids.ty else {
        panic!("expected a nilable union: {:?}", user_ids.ty)
    };
    assert!(
        variants.iter().any(|v| matches!(v, Ty::Array { elem } if matches!(**elem, Ty::Int))),
        "user_ids should be Array[Integer]: {variants:?}"
    );
}

/// A key passed an array at ONE call site and a scalar at another
/// resolves to the scalar: one helper, one signature, and the scalar
/// rendering is the one that was already shipping.
#[test]
fn a_mixed_array_and_scalar_option_stays_scalar() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", autocompletable_notes_path(user_ids: [ 1 ]) %>\n\
         <%= link_to \"b\", autocompletable_notes_path(user_ids: 2) %>\n",
    );
    let ps = params(&app, "autocompletable_notes_path");
    let user_ids = ps
        .iter()
        .find(|p| p.name.as_str() == "user_ids")
        .unwrap_or_else(|| panic!("no user_ids parameter: {ps:?}"));
    let Ty::Union { variants } = &user_ids.ty else {
        panic!("expected a nilable union: {:?}", user_ids.ty)
    };
    assert!(
        !variants.iter().any(|v| matches!(v, Ty::Array { .. })),
        "mixed usage must not claim an array: {variants:?}"
    );
}

/// The `_url` spelling is the same helper with a host in front, and its
/// rewrite happens after this survey — so the demand has to be recorded
/// against the `_path` helper that actually gets generated.
#[test]
fn the_url_spelling_feeds_the_path_helper() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", autocompletable_notes_url(room_id: 1) %>\n",
    );
    let ps = params(&app, "autocompletable_notes_path");
    assert_eq!(
        ps.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        vec!["room_id"],
        "the _url call site lands on the _path helper: {ps:?}"
    );
}
