//! Fixture YAML the way Psych and `ActiveRecord::FixtureSet::File` read
//! it: merge keys resolved, the `_fixture:` configuration row honoured,
//! and `test/fixtures/files/` left out.
//!
//! Shopify core is the forcing function: 131 of its fixture files build
//! rows by merging an anchored one (often in chains three or four deep),
//! and 1,532 open with `_fixture:` naming the model class and the
//! template rows to ignore. Before, a merged row failed as "fixture
//! field x.<< is not a scalar", and `_fixture` loaded as a row of its own.

use std::path::{Path, PathBuf};

use roundhouse::dialect::{Fixture, FixtureValue};
use roundhouse::ingest::{ingest_fixture_file, IngestError};
use roundhouse::Symbol;

fn fixture(yml: &str) -> Result<Fixture, IngestError> {
    ingest_fixture_file(
        yml.as_bytes(),
        Path::new("test/fixtures/things.yml"),
        Path::new("test/fixtures"),
    )
}

fn field(f: &Fixture, label: &str, key: &str) -> Option<String> {
    match f.records.get(&Symbol::from(label))?.get(&Symbol::from(key))? {
        FixtureValue::Scalar(s) => Some(s.clone()),
        FixtureValue::Ruby(_) => None,
    }
}

/// `c` merges `b`, which merges `a`. The copy of `b` under `c`'s `<<`
/// still has its own `<<`, so the innermost merge has to resolve first.
#[test]
fn a_merge_chain_resolves_innermost_first() {
    let f = fixture(
        "a: &a\n  name: A\n  color: red\n\
         b: &b\n  <<: *a\n  name: B\n  size: big\n\
         c:\n  <<: *b\n  name: C\n",
    )
    .expect("ingest");
    assert_eq!(field(&f, "c", "name").as_deref(), Some("C"));
    assert_eq!(field(&f, "c", "size").as_deref(), Some("big"));
    assert_eq!(field(&f, "c", "color").as_deref(), Some("red"));
    assert_eq!(field(&f, "b", "name").as_deref(), Some("B"));
    assert!(f.records.values().all(|r| !r.contains_key(&Symbol::from("<<"))));
}

/// `<<: [*a, *b]`: the earlier mapping wins a key both set.
#[test]
fn in_a_merge_list_the_earlier_mapping_wins() {
    let f = fixture("a: &a\n  name: A\nb: &b\n  name: B\n  size: big\nc:\n  <<: [*a, *b]\n")
        .expect("ingest");
    assert_eq!(field(&f, "c", "name").as_deref(), Some("A"));
    assert_eq!(field(&f, "c", "size").as_deref(), Some("big"));
}

/// `_fixture:` configures the set. Its `ignore` rows exist only to be
/// merged into others, so they are not inserted, but the rows merging
/// them keep their fields.
#[test]
fn fixture_config_is_not_a_row_and_ignored_rows_are_dropped() {
    let f = fixture(
        "_fixture:\n  model_class: Widget\n  ignore:\n    - DEFAULTS\n\
         DEFAULTS: &DEFAULTS\n  color: red\n\
         one:\n  <<: *DEFAULTS\n  name: One\n",
    )
    .expect("ingest");
    let labels: Vec<&str> = f.records.keys().map(|k| k.as_str()).collect();
    assert_eq!(labels, ["one"]);
    assert_eq!(field(&f, "one", "color").as_deref(), Some("red"));
    assert_eq!(f.model_class.as_ref().map(|c| c.as_str()), Some("Widget"));
}

/// A single `ignore: DEFAULTS` is a one-label list.
#[test]
fn a_scalar_ignore_names_one_label() {
    let f = fixture("_fixture:\n  ignore: DEFAULTS\nDEFAULTS: &d\n  a: 1\none:\n  <<: *d\n")
        .expect("ingest");
    let labels: Vec<&str> = f.records.keys().map(|k| k.as_str()).collect();
    assert_eq!(labels, ["one"]);
}

/// A redefined anchor: an alias after the second `&fr` means that one,
/// as in Psych. serde_yaml_ng on its own gives the redefinition the id of
/// the next new anchor (`&wallet` here), so `*fr` inside `wallet` reached
/// `wallet` itself and the parse recursed to its limit (core's
/// `payments/disputes.yml`).
#[test]
fn an_alias_after_a_redefined_anchor_takes_the_latest_definition() {
    let f = fixture(
        "base: &base\n  kind: dispute\n\
         fr: &fr\n  <<: *base\n  currency: EUR\n\
         fr_two: &fr\n  <<: *base\n  currency: EUR2\n\
         wallet: &wallet\n  <<: *fr\n  order: wallet\n\
         wallet_won:\n  <<: *wallet\n  status: won\n",
    )
    .expect("ingest");
    assert_eq!(field(&f, "wallet", "currency").as_deref(), Some("EUR2"));
    assert_eq!(field(&f, "wallet_won", "currency").as_deref(), Some("EUR2"));
    assert_eq!(field(&f, "wallet_won", "kind").as_deref(), Some("dispute"));
    assert_eq!(field(&f, "fr", "currency").as_deref(), Some("EUR"));
}

/// `b: &x` merging `*x` inside itself: Psych registers the anchor as the
/// node starts, so the merge is of the half-built row and adds nothing.
/// serde_yaml_ng on its own recursed to its depth limit (core's
/// `payments/transfers.yml`).
#[test]
fn an_alias_inside_its_own_anchor_merges_nothing() {
    let f = fixture(
        "a: &x\n  kind: first\n\
         b: &x\n  <<: *x\n  status: scheduled\n\
         c:\n  <<: *x\n  extra: 1\n",
    )
    .expect("ingest");
    assert_eq!(field(&f, "b", "kind"), None);
    assert_eq!(field(&f, "b", "status").as_deref(), Some("scheduled"));
    assert_eq!(field(&f, "c", "status").as_deref(), Some("scheduled"));
}

/// A row that sets a key twice takes the last value, as Psych does,
/// rather than failing the file.
#[test]
fn a_repeated_key_takes_its_last_value() {
    let f = fixture("one:\n  shop: first\n  shop: second\n").expect("ingest");
    assert_eq!(field(&f, "one", "shop").as_deref(), Some("second"));
}

/// A YAML file under `test/fixtures` that isn't label → fields is data a
/// test reads, not a fixture set. It is reported as such rather than as
/// a parse error of the file.
#[test]
fn a_sequence_at_the_top_is_not_a_fixture_set() {
    let err = fixture("- one\n- two\n").expect_err("a sequence is not a set");
    match err {
        IngestError::Unsupported { message, .. } => {
            assert!(message.contains("not a fixture set"), "{message}")
        }
        other => panic!("expected Unsupported, got {other}"),
    }
}

fn app(files: &[(&str, &str)]) -> roundhouse::app::App {
    let tree = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
                      create_table :widgets do |t|\n    t.string :name\n  end\nend\n";

/// `model_class` names the class the rows load, where the path would
/// name another (`legacy/things` -> `Legacy::Thing`, which does not
/// exist); without it the model is not found and every field drops.
#[test]
fn model_class_picks_the_model_the_rows_load() {
    let app = app(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("test/fixtures/legacy/things.yml", "_fixture:\n  model_class: Widget\none:\n  name: One\n"),
    ]);
    let lowered = roundhouse::lower::lower_fixtures(&app);
    let set = &lowered.fixtures[0];
    assert_eq!(set.class.0.as_str(), "Widget");
    assert_eq!(set.records[0].fields.len(), 1);
}

/// `test/fixtures/files/` is `file_fixture_path`; Rails leaves it out
/// of the fixture sets even when the files there are YAML.
#[test]
fn yaml_under_file_fixtures_is_not_a_fixture_set() {
    let app = app(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("test/fixtures/widgets.yml", "one:\n  name: One\n"),
        ("test/fixtures/files/payload.yml", "- a\n- b\n"),
    ]);
    let names: Vec<&str> = app.fixtures.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["widgets"]);
}

/// A hash or array value is for a JSON or serialized column. It is kept
/// as JSON text, which a `json` column stores as is and a YAML coder
/// reads back as the same value.
#[test]
fn a_nested_value_is_kept_as_json() {
    let f = fixture("one:\n  metadata:\n    source: web\n    tags: [a, b]\n  ids: [1, 2]\n")
        .expect("ingest");
    assert_eq!(field(&f, "one", "metadata").as_deref(), Some(r#"{"source":"web","tags":["a","b"]}"#));
    assert_eq!(field(&f, "one", "ids").as_deref(), Some("[1,2]"));
}

/// The JSON text reaches the emitted loader as a string literal.
#[test]
fn a_nested_value_is_emitted_as_a_string_literal() {
    let app = app(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  \
             create_table :widgets do |t|\n    t.json :metadata\n  end\nend\n",
        ),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("test/fixtures/widgets.yml", "one:\n  metadata:\n    source: web\n"),
    ]);
    let loader = roundhouse::emit::ruby::emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy() == "test/fixtures/widgets.rb")
        .map(|f| f.content)
        .expect("test/fixtures/widgets.rb");
    assert!(loader.contains(r#""{\"source\":\"web\"}""#), "{loader}");
}

/// An ERB tag inside a larger scalar builds the string at load time, as
/// ERB writing the tag's `to_s` into the text does.
#[test]
fn an_erb_tag_inside_a_scalar_interpolates() {
    let app = app(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("test/fixtures/widgets.yml", "one:\n  name: \"w-<%= 1 + 1 %> \\\"q\\\" #x\"\n"),
    ]);
    let loader = roundhouse::emit::ruby::emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy() == "test/fixtures/widgets.rb")
        .map(|f| f.content)
        .expect("test/fixtures/widgets.rb");
    assert!(loader.contains(r#""w-#{1 + 1} \"q\" \#x""#), "{loader}");
}

#[test]
fn anchor_like_text_in_scalars_and_comments_is_unchanged() {
    let f = fixture(
        "# &x is a comment\none:\n  name: 'literal &x token'\ntwo:\n  name: \"literal &x token\"\nthree:\n  name: literal *x token\nfour:\n  name: |\n    literal &x token\n    literal *x token\n",
    ).expect("ingest");
    assert_eq!(field(&f, "one", "name").as_deref(), Some("literal &x token"));
    assert_eq!(field(&f, "two", "name").as_deref(), Some("literal &x token"));
    assert_eq!(field(&f, "three", "name").as_deref(), Some("literal *x token"));
    assert_eq!(field(&f, "four", "name").as_deref(), Some("literal &x token\nliteral *x token\n"));
}

#[test]
fn inline_aliases_refer_to_completed_nodes_on_the_same_line() {
    let f = fixture("one: {name: &x hello, copy: *x, metadata: {a: &y [1, 2], b: *y}}\n")
        .expect("ingest");
    assert_eq!(field(&f, "one", "copy").as_deref(), Some("hello"));
    assert_eq!(field(&f, "one", "metadata").as_deref(), Some(r#"{"a":[1,2],"b":[1,2]}"#));
}

#[test]
fn anchor_names_cannot_collide_with_generated_names() {
    let f = fixture("a: &x {name: first}\nb: &x {name: second}\nc: &x__rh1 {name: third}\nd: {<<: *x}\n")
        .expect("ingest");
    assert_eq!(field(&f, "d", "name").as_deref(), Some("second"));
}

#[test]
fn recursive_values_are_reported_instead_of_becoming_empty_objects() {
    assert!(fixture("one: &x {metadata: *x}\n").is_err());
}

#[test]
fn plain_scalar_values_are_not_reparsed_as_documents() {
    let f = fixture("one: {name: ---, copy: ..., metadata: {number: 12, float: 1.5, flag: true, empty: null, string: 012}}\n")
        .expect("ingest");
    assert_eq!(field(&f, "one", "name").as_deref(), Some("---"));
    assert_eq!(field(&f, "one", "copy").as_deref(), Some("..."));
    assert_eq!(field(&f, "one", "metadata").as_deref(), Some(r#"{"number":12,"float":1.5,"flag":true,"empty":null,"string":"012"}"#));
}
