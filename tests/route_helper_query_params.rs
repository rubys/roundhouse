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

/// Neither `format:` nor `anchor:` is a QUERY key: `format` is a
/// trailing `.ext` on the path and `anchor` is the URL fragment. They
/// part ways after that — `format` becomes a keyword parameter that
/// appends the suffix (the `(.:format)` group is only ever spelled in
/// the `rails routes` shape, so a DSL-built route like campfire's has
/// to grow it from the call site), while `anchor` is still nobody's.
#[test]
fn format_is_a_suffix_keyword_and_anchor_is_neither() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", autocompletable_notes_path(format: :json, anchor: \"x\") %>\n",
    );
    let ps = params(&app, "autocompletable_notes_path");
    assert_eq!(
        ps.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        vec!["format"],
        "format takes a keyword slot, anchor takes none: {ps:?}"
    );
    assert!(matches!(ps[0].kind, ParamKind::Keyword { required: false }));
}

/// DEMAND-GATED, like the query keys beside it: a helper nobody passes
/// `format:` keeps exactly the signature it had, so no corpus call site
/// moves and no strict target grows a parameter to type.
#[test]
fn a_helper_nobody_formats_grows_no_format_param() {
    let app = app_with(ROUTES, "<%= link_to \"a\", autocompletable_notes_path %>\n");
    assert!(
        params(&app, "autocompletable_notes_path").is_empty(),
        "unformatted helper keeps its signature"
    );
}

/// An Array value renders as `k[]=a&k[]=b` in Rails (campfire writes
/// `rooms_directs_path(user_ids: [ user.id ])`). Left out rather than
/// rendered as the array's `to_s`: a wrong URL is worse than a missing
/// helper parameter.
#[test]
fn an_array_valued_option_declines() {
    let app = app_with(
        ROUTES,
        "<%= link_to \"a\", autocompletable_notes_path(user_ids: [ 1 ]) %>\n",
    );
    assert!(
        params(&app, "autocompletable_notes_path").is_empty(),
        "array values are not modeled"
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
