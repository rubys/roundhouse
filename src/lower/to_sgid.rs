//! `record.to_sgid(expires_in: nil, for: ActionText::Attachable::LOCATOR_NAME).to_s`
//! → `ActionText::SignedGlobalId.generate("Room", record.id)`.
//!
//! globalid's `to_sgid(**options)` mints a `SignedGlobalID` for any
//! record — the object ActionText's `attachable_sgid` calls it for
//! (`to_sgid(expires_in: nil, for: LOCATOR_NAME).to_s`, exactly this
//! spelling). The runtime already mints that envelope, byte for byte
//! Rails' (`ActionText::SignedGlobalId.generate`, verified against the
//! oracle in `runtime/ruby/test/action_text_test.rb`), and
//! `lower::attachable` bakes it into every attachable model as
//! `attachable_sgid`. What was missing is the call written OUT on a
//! record that is not attachable: campfire's
//! `action_text_attachment_test` wants an sgid for a `Room` — a
//! well-formed one it can then tamper with, to prove a bad signature
//! resolves to `MissingAttachable` — and the upstream test reached for
//! it by extending one live Room with `ActionText::Attachable`, which
//! no compiled target can do (`lower::object_extend`). Spelled as
//! `to_sgid(for:)`, the same sgid comes from a shape every target
//! compiles, and Rails computes the identical bytes for both.
//!
//! The model name is a compile-time fact, as `attachable.rs` and
//! `signed_id.rs` state: read off the receiver's static type, or — in a
//! test body, which is typed later than this pass runs — off a fixture
//! call (`rooms(:pets)` is a `Room`), or off a bare/`self` receiver in
//! a model body. A `for:` other than the attachable locator, an
//! `expires_in:` other than nil, a missing `.to_s`, or a receiver whose
//! model the pass cannot name is reported and left as written — the
//! runtime has no `to_sgid`, so the site fails loudly with the reason
//! rather than minting an sgid for a purpose nothing verifies.

use crate::app::App;
use crate::diagnostic::{Diagnostic, Severity};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;
use std::collections::BTreeMap;

/// `ActionText::Attachable::LOCATOR_NAME`, as the runtime's
/// `SignedGlobalId::PURPOSE` spells it.
const LOCATOR_NAME: &str = "attachable";

pub fn apply_to_sgid_lowering(app: &mut App) -> Vec<Diagnostic> {
    let fixtures: BTreeMap<String, String> = app
        .fixtures
        .iter()
        .map(|f| (f.name.as_str().to_string(), crate::naming::classify_path(f.path.as_str())))
        .collect();
    let mut diags = Vec::new();
    super::for_each_model_body_named(app, &mut |model, body| {
        rewrite(body, Some(model), &fixtures, &mut diags)
    });
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, None, &fixtures, &mut diags);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, None, &fixtures, &mut diags);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, None, &fixtures, &mut diags);
        }
    }
    diags
}

fn rewrite(e: &mut Expr, model: Option<&str>, fixtures: &BTreeMap<String, String>, diags: &mut Vec<Diagnostic>) {
    // Top-down: the `.to_s` wraps the `to_sgid`, and the pair is read
    // as one shape. A pair the pass declines is reported once, here,
    // and the walk goes on beneath the `to_sgid` rather than into it.
    if is_sgid_to_s(e) {
        if let Some(replacement) = rewritten(e, model, fixtures, diags) {
            *e = replacement;
            return;
        }
        let ExprNode::Send { recv: Some(inner), .. } = &mut *e.node else { unreachable!() };
        inner.node.for_each_child_mut(&mut |c| rewrite(c, model, fixtures, diags));
        return;
    }
    if let ExprNode::Send { method, .. } = &*e.node {
        if method.as_str() == "to_sgid" {
            decline(e, diags);
        }
    }
    e.node.for_each_child_mut(&mut |c| rewrite(c, model, fixtures, diags));
}

/// `<x>.to_sgid(…).to_s`.
fn is_sgid_to_s(e: &Expr) -> bool {
    let ExprNode::Send { recv: Some(inner), method, args, block: None, .. } = &*e.node else { return false };
    if method.as_str() != "to_s" || !args.is_empty() {
        return false;
    }
    matches!(&*inner.node, ExprNode::Send { method: m, block: None, .. } if m.as_str() == "to_sgid")
}

fn rewritten(
    e: &Expr,
    model: Option<&str>,
    fixtures: &BTreeMap<String, String>,
    diags: &mut Vec<Diagnostic>,
) -> Option<Expr> {
    let ExprNode::Send { recv: Some(inner), .. } = &*e.node else { return None };
    let ExprNode::Send { recv, args: sgid_args, .. } = &*inner.node else { return None };
    let span = e.span;
    let report = |why: &str, diags: &mut Vec<Diagnostic>| {
        let mut d = Diagnostic::unsupported(
            span,
            None,
            "to_sgid",
            format!("`to_sgid` is served only as `to_sgid(expires_in: nil, for: ActionText::Attachable::LOCATOR_NAME).to_s` on a record whose model is known here: {why}"),
        );
        d.severity = Severity::Warning;
        diags.push(d);
    };
    // The options: `for:` the attachable locator, `expires_in:` nil or
    // absent, nothing else.
    let mut purpose_ok = false;
    match sgid_args.as_slice() {
        [opts] => {
            let ExprNode::Hash { entries, .. } = &*opts.node else {
                report("its options are not keywords", diags);
                return None;
            };
            for (k, v) in entries {
                let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                    report("its options are not keywords", diags);
                    return None;
                };
                match key.as_str() {
                    "for" => {
                        if is_locator_name(v) {
                            purpose_ok = true;
                        } else {
                            report("its `for:` is not the attachable locator", diags);
                            return None;
                        }
                    }
                    "expires_in" => {
                        if !matches!(&*v.node, ExprNode::Lit { value: Literal::Nil }) {
                            report("its `expires_in:` is not nil (Rails' attachable sgids do not expire)", diags);
                            return None;
                        }
                    }
                    other => {
                        report(&format!("`{other}:` is not an option this reproduces"), diags);
                        return None;
                    }
                }
            }
        }
        [] => {}
        _ => {
            report("its options are not keywords", diags);
            return None;
        }
    }
    if !purpose_ok {
        report("it names no `for:` (an unpurposed sgid is verified by nothing here)", diags);
        return None;
    }
    let (model_name, record) = match recv {
        None => (model.map(str::to_string), self_ref(span)),
        Some(r) => (model_of(r, model, fixtures), r.clone()),
    };
    let Some(model_name) = model_name else {
        report("the receiver's model is not known", diags);
        return None;
    };
    let mut id_read = Expr::new(span, ExprNode::Send {
        recv: Some(record),
        method: Symbol::from("id"),
        args: vec![],
        block: None,
        parenthesized: false,
    });
    id_read.ty = Some(Ty::Int);
    let mut name_lit = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: model_name } });
    name_lit.ty = Some(Ty::Str);
    let mut out = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("ActionText"), Symbol::from("SignedGlobalId")] },
            )),
            method: Symbol::from("generate"),
            args: vec![name_lit, id_read],
            block: None,
            parenthesized: true,
        },
    );
    out.ty = Some(Ty::Str);
    Some(out)
}

/// A bare `to_sgid` send (no `.to_s` above it) is reported and left.
fn decline(e: &Expr, diags: &mut Vec<Diagnostic>) {
    let mut d = Diagnostic::unsupported(
        e.span,
        None,
        "to_sgid",
        "`to_sgid` is served only as `to_sgid(expires_in: nil, for: ActionText::Attachable::LOCATOR_NAME).to_s`: a SignedGlobalID object is not a value this runtime carries, only its String",
    );
    d.severity = Severity::Warning;
    diags.push(d);
}

/// `ActionText::Attachable::LOCATOR_NAME`, or the literal it is.
fn is_locator_name(v: &Expr) -> bool {
    match &*v.node {
        ExprNode::Const { path } => {
            path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::") == "ActionText::Attachable::LOCATOR_NAME"
        }
        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str() == LOCATOR_NAME,
        ExprNode::Lit { value: Literal::Str { value } } => value == LOCATOR_NAME,
        _ => false,
    }
}

/// The model a receiver is: its static type when the analyzer stamped
/// one, a fixture call's model, or `self` in a model body.
fn model_of(r: &Expr, model: Option<&str>, fixtures: &BTreeMap<String, String>) -> Option<String> {
    if let Some(Ty::Class { id, .. }) = r.ty.as_ref().map(|t| t.clone().strip_nil()) {
        return Some(id.0.as_str().to_string());
    }
    match &*r.node {
        ExprNode::SelfRef => model.map(str::to_string),
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.len() == 1 => {
            fixtures.get(method.as_str()).cloned()
        }
        _ => None,
    }
}

fn self_ref(span: Span) -> Expr {
    Expr::new(span, ExprNode::SelfRef)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp() -> Span {
        Span::synthetic()
    }
    fn sym(s: &str) -> Expr {
        Expr::new(sp(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
    }
    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(sp(), ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true })
    }
    fn locator() -> Expr {
        Expr::new(sp(), ExprNode::Const { path: ["ActionText", "Attachable", "LOCATOR_NAME"].iter().map(|s| Symbol::from(*s)).collect() })
    }
    fn opts(entries: Vec<(&str, Expr)>) -> Expr {
        Expr::new(sp(), ExprNode::Hash { entries: entries.into_iter().map(|(k, v)| (sym(k), v)).collect(), kwargs: true })
    }
    fn nil() -> Expr {
        Expr::new(sp(), ExprNode::Lit { value: Literal::Nil })
    }
    fn fixtures() -> BTreeMap<String, String> {
        [("rooms".to_string(), "Room".to_string())].into_iter().collect()
    }

    #[test]
    fn a_fixture_records_attachable_sgid_spelled_out_becomes_the_runtime_mint() {
        // rooms(:pets).to_sgid(expires_in: nil, for: ActionText::Attachable::LOCATOR_NAME).to_s
        let record = send(None, "rooms", vec![sym("pets")]);
        let sgid = send(Some(record), "to_sgid", vec![opts(vec![("expires_in", nil()), ("for", locator())])]);
        let mut e = send(Some(sgid), "to_s", vec![]);
        let mut diags = Vec::new();
        rewrite(&mut e, None, &fixtures(), &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
        let ExprNode::Send { recv: Some(k), method, args, .. } = &*e.node else { panic!("{:?}", e.node) };
        assert!(matches!(&*k.node, ExprNode::Const { path } if path.len() == 2 && path[1].as_str() == "SignedGlobalId"));
        assert_eq!(method.as_str(), "generate");
        assert!(matches!(&*args[0].node, ExprNode::Lit { value: Literal::Str { value } } if value == "Room"));
        assert!(matches!(&*args[1].node, ExprNode::Send { method, .. } if method.as_str() == "id"));
        assert_eq!(e.ty, Some(Ty::Str));
    }

    #[test]
    fn another_purpose_is_reported_and_left() {
        let record = send(None, "rooms", vec![sym("pets")]);
        let sgid = send(Some(record), "to_sgid", vec![opts(vec![("for", sym("transfer"))])]);
        let mut e = send(Some(sgid), "to_s", vec![]);
        let mut diags = Vec::new();
        rewrite(&mut e, None, &fixtures(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("not the attachable locator"), "{}", diags[0].message);
        assert!(matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "to_s"));
    }

    #[test]
    fn a_sgid_kept_as_an_object_is_reported() {
        // x = rooms(:pets).to_sgid(for: …)   — no .to_s
        let record = send(None, "rooms", vec![sym("pets")]);
        let mut e = send(Some(record), "to_sgid", vec![opts(vec![("for", locator())])]);
        let mut diags = Vec::new();
        rewrite(&mut e, None, &fixtures(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("only its String"));
    }
}
