//! Kernel methods every object answers, and the third builtin
//! constructor — three gaps that all read the same way on the ledger.
//!
//! 1. **`then` / `yield_self` were not universal.** `tap` was, in the
//!    receiver-agnostic table; its mirror was nowhere. So a chain
//!    through `Opengraph::Location.new(url).then { |l| … }` reported
//!    `no known method then on Class(Opengraph::Location)` — against a
//!    class that answers it because EVERY class does.
//!
//! 2. **The block parameter of `then` / `yield_self` / `tap` was
//!    unbound.** These yield the RECEIVER, on every type, and
//!    `block_params_for` matched on the receiver's SHAPE first, so a
//!    class receiver never reached an arm. That made the inside of
//!    every `tap` block a blind spot: campfire's `Net::HTTP.new(…).tap
//!    { |http| http.use_ssl = … }` was silent about four sends the
//!    emit cannot lower, and the silence was the parameter having no
//!    type rather than the sends being fine.
//!
//! 3. **`String.new` did not answer `Str`.** `Hash.new` and
//!    `Array.new` mapped to their parameterized IR types so that
//!    subsequent calls dispatch through the container tables; the
//!    third builtin constructor did not, so `String.new(body)
//!    .force_encoding("UTF-8")` looked for `force_encoding` in a class
//!    table with no methods in it. `force_encoding` was missing from
//!    `str_method` besides — the two halves had to land together or the
//!    error just moves.

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n    t.string :bio\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  resources :users\nend\n";

fn errors(extra: Vec<(&str, &str)>) -> Vec<String> {
    let mut tree: Vec<(&str, &str)> =
        vec![("db/schema.rb", SCHEMA), ("config/routes.rb", ROUTES)];
    tree.extend(extra);
    let tree = tree
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error"))
        .collect()
}

const LOCATION: &str = "class Location\n  \
    def initialize(url)\n    @url = url\n  end\n  \
    def read_html\n    \"<html>\"\n  end\nend\n";

#[test]
fn then_resolves_on_any_receiver_and_yields_it() {
    let found = errors(vec![
        ("app/models/location.rb", LOCATION),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  \
               def fetched\n    \
                 Location.new(bio).then do |location|\n      \
                   location.read_html.upcase\n    \
                 end\n  \
               end\nend\n",
        ),
    ]);
    assert!(
        found.is_empty(),
        "`then` is Kernel's, and its block parameter is the receiver: {found:?}"
    );
}

/// The binding half on its own, and it has to be asserted in the
/// POSITIVE direction. An unbound block parameter does not produce a
/// diagnostic — it makes the parameter's type unknown, and a send whose
/// receiver is unknown is deliberately skipped so the root cause is
/// reported once, on the receiver. So "clean ledger" is what a blind
/// spot looks like too, and a test asserting cleanliness here passes
/// with the fix removed.
///
/// What changed is the opposite: a send the receiver genuinely cannot
/// answer is now REPORTED where it used to pass invisibly. campfire's
/// `Net::HTTP.new(…).tap { |http| http.use_ssl = … }` is the shape —
/// four sends the emit cannot lower, silent until the parameter had a
/// type.
#[test]
fn tap_binds_its_block_parameter_to_the_receiver() {
    let found = errors(vec![
        ("app/models/location.rb", LOCATION),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  \
               def fetched\n    \
                 Location.new(bio).tap do |location|\n      \
                   location.read_html\n      \
                   location.no_such_method\n    \
                 end\n  \
               end\nend\n",
        ),
    ]);
    assert!(
        found.iter().any(|d| d.contains("`no_such_method`")),
        "the tap parameter is the receiver, so what it cannot answer must be reported: {found:?}"
    );
    assert!(
        !found.iter().any(|d| d.contains("`read_html`")),
        "and what it CAN answer must not be: {found:?}"
    );
}

#[test]
fn string_new_is_a_str_and_answers_force_encoding() {
    let found = errors(vec![(
        "app/models/user.rb",
        "class User < ApplicationRecord\n  \
           def utf8_bio\n    \
             String.new(bio).force_encoding(\"UTF-8\")\n  \
           end\nend\n",
    )]);
    assert!(
        found.is_empty(),
        "`String.new` is a Str and Str answers `force_encoding`: {found:?}"
    );
}

/// `lower::blank` rewrites `compact_blank` to a `reject` and re-stamps
/// the receiver as `Array[non_nil(elem)]`. The analyzer had no arm for
/// the name at all, so the send READING that receiver kept the
/// analyzer's answer — nothing — and campfire's `[ name, bio ]
/// .compact_blank.join(" - ")` reported `no known method join on
/// Array[Str]`, against a receiver the lowering had just typed right.
/// The two must agree, and the analyzer is where the agreement starts.
#[test]
fn compact_blank_on_an_array_keeps_its_element_type() {
    let found = errors(vec![(
        "app/models/user.rb",
        "class User < ApplicationRecord\n  \
           def title\n    \
             [ name, bio ].compact_blank.join(\" - \")\n  \
           end\nend\n",
    )]);
    assert!(
        found.is_empty(),
        "compact_blank drops the nil half and keeps the rest: {found:?}"
    );
}
