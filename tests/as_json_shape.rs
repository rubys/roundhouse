//! `lower::as_json_shape` — recognizing a model's `as_json` body as an
//! ordered pair list, the analysis half of monomorphizing inline
//! `render json:`.
//!
//! Bodies are the real lobsters ones (Story, Comment, User, Message),
//! trimmed to the statements that shape the result. They are the whole
//! reason the pass exists: every one of them COMPUTES its key set
//! rather than declaring it, and two of them make that set depend on
//! the row.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::as_json_shape::{as_json_pairs, JsonPair, PairValue};

fn app_from(files: Vec<(&str, &str)>) -> roundhouse::App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

/// Pull `<Model>#as_json`'s body out of an ingested app.
fn as_json_body(app: &roundhouse::App, model_name: &str) -> roundhouse::expr::Expr {
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == model_name)
        .unwrap_or_else(|| panic!("no model {model_name}"));
    for item in &model.body {
        if let roundhouse::dialect::ModelBodyItem::Method { method, .. } = item {
            if method.name.as_str() == "as_json" {
                return method.body.clone();
            }
        }
    }
    panic!("{model_name} has no as_json");
}

fn keys(pairs: &[JsonPair]) -> Vec<String> {
    pairs.iter().map(|p| p.key.as_str().to_string()).collect()
}

fn find<'a>(pairs: &'a [JsonPair], key: &str) -> &'a JsonPair {
    pairs
        .iter()
        .find(|p| p.key.as_str() == key)
        .unwrap_or_else(|| panic!("no pair {key}"))
}

// ── idiom A: entry list + walk ─────────────────────────────────────

#[test]
fn entry_list_reads_bare_symbols_and_renames_in_source_order() {
    // Comment's shape: bare symbols, a `{ key: :other_reader }` rename,
    // and a `{ key: <expr> }` computed value.
    let app = app_from(vec![(
        "app/models/comment.rb",
        "class Comment < ApplicationRecord\n\
         \x20 def as_json(_options = {})\n\
         \x20   h = [\n\
         \x20     :short_id,\n\
         \x20     :score,\n\
         \x20     { :commenting_user => :user },\n\
         \x20     { :tags => self.tags.map { |t| t.tag } },\n\
         \x20   ]\n\
         \x20   js = {}\n\
         \x20   h.each do |k|\n\
         \x20     js[k] = self.send(k)\n\
         \x20   end\n\
         \x20   js\n\
         \x20 end\n\
         end\n",
    )]);
    let pairs = as_json_pairs(&as_json_body(&app, "Comment")).expect("recognized");

    // Source order is the JSON order — Rails emits in insertion order,
    // so a reordering here would be a visible diff against Rails.
    assert_eq!(keys(&pairs), ["short_id", "score", "commenting_user", "tags"]);

    // A bare symbol is key AND reader.
    assert!(matches!(&find(&pairs, "short_id").value,
        PairValue::Reader(r) if r.as_str() == "short_id"));

    // A rename changes only the KEY: the value still comes from a
    // reader, the differently-named one. This is the `is_a?(Symbol)`
    // test the runtime walk performs, answered statically.
    assert!(matches!(&find(&pairs, "commenting_user").value,
        PairValue::Reader(r) if r.as_str() == "user"));

    // A non-symbol hash value is the value itself, used verbatim.
    assert!(matches!(&find(&pairs, "tags").value, PairValue::Computed(_)));

    // Nothing in this idiom is row-dependent.
    assert!(pairs.iter().all(|p| p.cond.is_none()));
}

#[test]
fn entry_list_extended_by_push_is_declined() {
    // Story guards `h.push(comments: options[:with_comments])` on its
    // options argument. The key set is then argument-dependent, which
    // this analysis cannot settle — proving `options == {}` belongs to
    // the call-site specializer. Decline rather than emit a key set
    // that is wrong the moment options are passed.
    let app = app_from(vec![(
        "app/models/story.rb",
        "class Story < ApplicationRecord\n\
         \x20 def as_json(options = {})\n\
         \x20   h = [ :short_id, :title ]\n\
         \x20   h.push(:comments => options[:with_comments]) if options && options[:with_comments]\n\
         \x20   js = {}\n\
         \x20   h.each do |k|\n\
         \x20     js[k] = self.send(k)\n\
         \x20   end\n\
         \x20   js\n\
         \x20 end\n\
         end\n",
    )]);
    let err = as_json_pairs(&as_json_body(&app, "Story")).expect_err("declined");
    assert!(err.contains("push"), "error should name the blocking construct, got: {err}");
}

// ── idiom B: attrs + super(only:) + post-hoc writes ────────────────

#[test]
fn attrs_idiom_keeps_a_row_dependent_key_as_a_condition() {
    // User's shape. `karma` is present only for non-admins, so the key
    // set genuinely varies per row — the pass must NOT flatten that
    // away, or the emitted JSON stops matching Rails for half the rows.
    let app = app_from(vec![(
        "app/models/user.rb",
        "class User < ApplicationRecord\n\
         \x20 def as_json(_options = {})\n\
         \x20   attrs = [\n\
         \x20     :username,\n\
         \x20     :created_at,\n\
         \x20   ]\n\
         \x20   if !self.is_admin?\n\
         \x20     attrs.push :karma\n\
         \x20   end\n\
         \x20   attrs.push :homepage, :about\n\
         \x20   h = super(:only => attrs)\n\
         \x20   h[:avatar_url] = self.avatar_url\n\
         \x20   h\n\
         \x20 end\n\
         end\n",
    )]);
    let pairs = as_json_pairs(&as_json_body(&app, "User")).expect("recognized");

    assert_eq!(
        keys(&pairs),
        ["username", "created_at", "karma", "homepage", "about", "avatar_url"]
    );

    // The row-dependent one carries its guard; its neighbours do not.
    assert!(find(&pairs, "karma").cond.is_some(), "karma must stay conditional");
    assert!(find(&pairs, "username").cond.is_none());
    assert!(find(&pairs, "homepage").cond.is_none(), "multi-arg push is unconditional here");

    // A post-`super` write contributes a computed pair.
    assert!(matches!(&find(&pairs, "avatar_url").value, PairValue::Computed(_)));
}

#[test]
fn attrs_idiom_handles_a_body_with_no_conditional_keys() {
    // Message's shape — same idiom, nothing row-dependent.
    let app = app_from(vec![(
        "app/models/message.rb",
        "class Message < ApplicationRecord\n\
         \x20 def as_json(_options = {})\n\
         \x20   attrs = [ :short_id, :subject ]\n\
         \x20   h = super(:only => attrs)\n\
         \x20   h[:author_username] = self.author.username\n\
         \x20   h\n\
         \x20 end\n\
         end\n",
    )]);
    let pairs = as_json_pairs(&as_json_body(&app, "Message")).expect("recognized");
    assert_eq!(keys(&pairs), ["short_id", "subject", "author_username"]);
    assert!(pairs.iter().all(|p| p.cond.is_none()));
}

#[test]
fn an_unrecognized_body_is_declined_rather_than_guessed() {
    // No entry list at all — the caller must ledger this and leave the
    // respond_to arm dropped, which is today's behavior. Degrading to
    // "no JSON" is correct; degrading to "wrong JSON" is not.
    let app = app_from(vec![(
        "app/models/widget.rb",
        "class Widget < ApplicationRecord\n\
         \x20 def as_json(_options = {})\n\
         \x20   build_it_somehow\n\
         \x20 end\n\
         end\n",
    )]);
    assert!(as_json_pairs(&as_json_body(&app, "Widget")).is_err());
}

