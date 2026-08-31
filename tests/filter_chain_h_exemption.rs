//! The Action Text filter-chain escape exemption — see the module note
//! in `src/lower/html_safe.rs`.
//!
//! campfire's `Filter.apply` wraps every filter product in
//! `ActionText::Content.new(...)`, whose `to_s` is born html-safe in
//! Rails; `h(<chain>.apply(...))` therefore passes markup through. No
//! runtime here carries that mark (the shared runtime has no
//! safe-buffer type by design), so without the rewrite the sanitized
//! body rendered as its own escaped source — on BOTH lanes, invisible
//! until a browser posted an HTML body through Trix.
//!
//! Asserted on the LOWERED IR rather than an emit: an unreferenced
//! helper module is tree-shaken out of the emitted file set, and what
//! this file gates is the rewrite itself, which every emitter then
//! prints as written.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::expr::{Expr, ExprNode};
use roundhouse::ingest::ingest_app_from_tree;

fn lowered_helper_body(helper_src: &str) -> Expr {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/helpers/content_filters.rb"),
        b"module ContentFilters\n  TextMessagePresentationFilters = ActionText::Content::Filters.new(SanitizeTags, SanitizeAttributes)\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/helpers/messages_helper.rb"),
        helper_src.as_bytes().to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app.library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == "MessagesHelper")
        .expect("MessagesHelper not ingested")
        .methods
        .iter()
        .find(|m| m.name.as_str() == "message_presentation")
        .expect("message_presentation not found")
        .body
        .clone()
}

fn any_expr(e: &Expr, pred: &mut impl FnMut(&Expr) -> bool) -> bool {
    if pred(e) {
        return true;
    }
    let mut found = false;
    let mut probe = e.clone();
    probe.node.for_each_child_mut(&mut |c| {
        if !found && any_expr(c, pred) {
            found = true;
        }
    });
    found
}

fn is_to_s_of_chain_apply(e: &Expr) -> bool {
    let ExprNode::Send { method, recv: Some(recv), .. } = &*e.node else { return false };
    if method.as_str() != "to_s" {
        return false;
    }
    let ExprNode::Send { method: inner, .. } = &*recv.node else { return false };
    inner.as_str() == "apply"
}

fn is_h_call(e: &Expr) -> bool {
    let ExprNode::Send { method, .. } = &*e.node else { return false };
    method.as_str() == "h"
}

#[test]
fn h_of_a_filter_chain_apply_becomes_to_s() {
    let body = lowered_helper_body(
        "module MessagesHelper\n  def message_presentation(message)\n    auto_link h(ContentFilters::TextMessagePresentationFilters.apply(message)), html: { target: \"_blank\" }\n  end\nend\n",
    );
    assert!(
        any_expr(&body, &mut is_to_s_of_chain_apply),
        "the chain's product should stringify instead of being escaped:\n{body:?}"
    );
    assert!(
        !any_expr(&body, &mut is_h_call),
        "the h() wrapper should be gone:\n{body:?}"
    );
}

#[test]
fn h_of_anything_else_is_untouched() {
    let body = lowered_helper_body(
        "module MessagesHelper\n  def message_presentation(message)\n    h(Formatter.apply(message))\n  end\nend\n",
    );
    assert!(
        any_expr(&body, &mut is_h_call),
        "an apply on a non-chain constant keeps its escape:\n{body:?}"
    );
    assert!(
        !any_expr(&body, &mut is_to_s_of_chain_apply),
        "no to_s rewrite should have fired:\n{body:?}"
    );
}
