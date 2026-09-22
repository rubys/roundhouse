//! `ActionCable::Channel::TestCase` bodies, spelled for the harness in
//! `runtime/spinel/test/test_helper.rb` — plus the two class-level
//! Turbo calls a channel test reaches for.
//!
//! Rails' channel test case resolves the class under test by NAME
//! (`tests RoomMessagesChannel`, or the test class's name minus `Test`),
//! builds it with `channel_class.new(connection, identifier, params)`,
//! and answers `assert_has_stream_for record` with
//! `channel_class.broadcasting_for(record)`. Every one of those is a
//! class object reached at run time, which a target that resolves calls
//! statically has no lane for — so the name is resolved HERE, once, and
//! written into the call:
//!
//! ```ruby
//! subscribe room_id: room.id          # → subscribe_to("PresenceChannel", ["room_id"], [room.id.to_s])
//! subscribe                           # → subscribe_to("UnreadRoomsChannel", [], [])
//! assert_has_stream_for room          # → assert_has_stream(PresenceChannel.broadcasting_for(room))
//! ```
//!
//! `subscribe_to` builds the channel through `ActionCable::Channel
//! .build`, the generated factory `Cable.subscribe` resolves a client's
//! frame through, so the test asks the runtime what a browser would.
//! The values ride as STRINGS: a frame's `params` read as strings on
//! every lane (`ActionCable::Channel::Parameters`), and Rails' own
//! `params` are string-valued one hop earlier for the same reason.
//!
//! The Turbo pair is what a test hands the channel:
//!
//! ```ruby
//! Turbo::StreamsChannel.signed_stream_name([@room, :messages])
//!   # → Turbo::Streams::StreamName.signed("#{@room.to_gid_param}:messages")
//! ```
//!
//! turbo-rails' `stream_name_from` is `streamable.try(:to_gid_param) ||
//! streamable.to_param`, joined by `:` — the convention
//! `lower::broadcasts::stream_name` spells for a view, restated for a
//! test body where the record's model is not a compile-time fact but
//! its `to_gid_param` is (every model carries one). A Symbol or String
//! literal contributes its own text; anything else is taken for a
//! record. `Turbo.signed_stream_verifier.verified(x)` needs no rewrite:
//! the runtime's `Turbo.signed_stream_verifier` is a real object.
//!
//! A `subscribe` whose channel cannot be named, or whose argument is
//! not a literal keyword hash, is reported and left as written — the
//! harness has no `subscribe`, so the test fails loudly with the reason
//! rather than subscribing to the wrong channel.

use crate::app::App;
use crate::diagnostic::{Diagnostic, Severity};
use crate::dialect::TestModule;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use std::collections::BTreeSet;

const CHANNEL_TEST_CASE: &str = "ActionCable::Channel::TestCase";
/// The one channel that is a runtime class rather than an app one, and
/// so is never in `library_classes` — `Channel.build` always carries an
/// arm for it.
const STOCK_CHANNEL: &str = "Turbo::StreamsChannel";

pub fn apply_cable_test_case_lowering(app: &mut App) -> Vec<Diagnostic> {
    let channels: BTreeSet<String> = app
        .library_classes
        .iter()
        .filter(|lc| !lc.is_module)
        .map(|lc| lc.name.0.as_str().to_string())
        .chain(std::iter::once(STOCK_CHANNEL.to_string()))
        .collect();
    let mut diags = Vec::new();
    for tm in &mut app.test_modules {
        let channel = if tm.parent.as_ref().is_some_and(|p| p.0.as_str() == CHANNEL_TEST_CASE) {
            channel_under_test(tm, &channels)
        } else {
            None
        };
        let mut rewrite = |e: &mut Expr| rewrite(e, channel.as_deref(), &mut diags);
        if let Some(setup) = &mut tm.setup {
            rewrite(setup);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body);
        }
    }
    diags
}

/// Rails' `channel_class`: the `tests` macro's argument (ingested into
/// `target`), else the test class's name with its `Test` suffix
/// removed — only when that names a class the factory has an arm for.
fn channel_under_test(tm: &TestModule, channels: &BTreeSet<String>) -> Option<String> {
    let candidate = match &tm.target {
        Some(t) => t.0.as_str().to_string(),
        None => tm.name.0.as_str().strip_suffix("Test")?.to_string(),
    };
    channels.contains(&candidate).then_some(candidate)
}

fn rewrite(e: &mut Expr, channel: Option<&str>, diags: &mut Vec<Diagnostic>) {
    e.node.for_each_child_mut(&mut |c| rewrite(c, channel, diags));
    let span = e.span;
    let ExprNode::Send { recv, method, args, block, .. } = &mut *e.node else { return };
    match (recv.as_ref(), method.as_str()) {
        (None, "subscribe") if block.is_none() => {
            let Some(channel) = channel else {
                report(span, "subscribe", "the channel under test could not be named: no `tests` line and the class name minus `Test` is not a channel in the tree", diags);
                return;
            };
            let Some((keys, values)) = subscribe_params(args) else {
                report(span, "subscribe", "its params are not a literal keyword hash", diags);
                return;
            };
            *e = send(span, None, "subscribe_to", vec![str_lit(span, channel), keys, values]);
        }
        (None, "assert_has_stream_for") if args.len() == 1 && block.is_none() => {
            let Some(channel) = channel else {
                report(span, "assert_has_stream_for", "the channel under test could not be named", diags);
                return;
            };
            let record = args.remove(0);
            let broadcasting = send(
                span,
                Some(const_path(span, channel)),
                "broadcasting_for",
                vec![record],
            );
            *e = send(span, None, "assert_has_stream", vec![broadcasting]);
        }
        (Some(r), "signed_stream_name") if args.len() == 1 && is_const(r, STOCK_CHANNEL) => {
            let Some(name) = stream_name_of(&args[0]) else {
                report(span, "signed_stream_name", "its streamables are not an array literal", diags);
                return;
            };
            *e = send(
                span,
                Some(const_path(span, "Turbo::Streams::StreamName")),
                "signed",
                vec![name],
            );
        }
        _ => {}
    }
}

/// `[ ]` / `[ k: v, … ]` → the keys and the values as two String-array
/// literals. Keys must be Symbol or String literals; values are `.to_s`
/// of what was written.
fn subscribe_params(args: &[Expr]) -> Option<(Expr, Expr)> {
    let span = Span::synthetic();
    let mut keys = Vec::new();
    let mut values = Vec::new();
    match args {
        [] => {}
        [opts] => {
            let ExprNode::Hash { entries, .. } = &*opts.node else { return None };
            for (k, v) in entries {
                let key = match &*k.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                    ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                    _ => return None,
                };
                keys.push(str_lit(span, &key));
                values.push(send(v.span, Some(v.clone()), "to_s", vec![]));
            }
        }
        _ => return None,
    }
    Some((array(span, keys), array(span, values)))
}

/// turbo-rails' `stream_name_from` over an array literal: literals
/// contribute their text, everything else its `to_gid_param`, joined
/// by `:`. A single record with no literal beside it is a bare
/// `to_gid_param` call rather than a one-part interpolation.
fn stream_name_of(streamables: &Expr) -> Option<Expr> {
    let ExprNode::Array { elements, .. } = &*streamables.node else { return None };
    if elements.is_empty() {
        return None;
    }
    let mut parts: Vec<InterpPart> = Vec::new();
    let mut pending = String::new();
    for (i, el) in elements.iter().enumerate() {
        if i > 0 {
            pending.push(':');
        }
        match &*el.node {
            ExprNode::Lit { value: Literal::Sym { value } } => pending.push_str(value.as_str()),
            ExprNode::Lit { value: Literal::Str { value } } => pending.push_str(value),
            _ => {
                if !pending.is_empty() {
                    parts.push(InterpPart::Text { value: std::mem::take(&mut pending) });
                }
                parts.push(InterpPart::Expr {
                    expr: send(el.span, Some(el.clone()), "to_gid_param", vec![]),
                });
            }
        }
    }
    if !pending.is_empty() {
        parts.push(InterpPart::Text { value: pending });
    }
    let span = streamables.span;
    Some(match parts.as_slice() {
        [InterpPart::Text { value }] => str_lit(span, value),
        [InterpPart::Expr { expr }] => expr.clone(),
        _ => Expr::new(span, ExprNode::StringInterp { parts }),
    })
}

fn is_const(e: &Expr, path: &str) -> bool {
    let ExprNode::Const { path: p } = &*e.node else { return false };
    p.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::") == path
}

fn report(span: Span, construct: &str, why: &str, diags: &mut Vec<Diagnostic>) {
    let mut d = Diagnostic::unsupported(
        span,
        None,
        construct,
        format!("`{construct}` in an ActionCable::Channel::TestCase is served only with the channel resolved at lower time: {why}"),
    );
    d.severity = Severity::Warning;
    diags.push(d);
}

fn send(span: Span, recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn const_path(span: Span, path: &str) -> Expr {
    Expr::new(span, ExprNode::Const { path: path.split("::").map(Symbol::from).collect() })
}

fn str_lit(span: Span, value: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.to_string() } })
}

fn array(span: Span, elements: Vec<Expr>) -> Expr {
    Expr::new(span, ExprNode::Array { elements, style: Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::ClassId;

    fn test_module(name: &str, parent: &str, target: Option<&str>, body: &str) -> TestModule {
        let src = format!(
            "class {name} < {parent}\n  {}\n  test \"t\" do\n    {body}\n  end\nend\n",
            target.map(|t| format!("tests {t}")).unwrap_or_default()
        );
        let mut mods = crate::ingest::test::ingest_test_files(src.as_bytes(), "t.rb").expect("ingest");
        assert_eq!(mods.len(), 1);
        mods.remove(0)
    }

    fn app_with(tm: TestModule, channels: &[&str]) -> App {
        let mut app = App::new();
        for c in channels {
            app.library_classes.push(crate::dialect::LibraryClass {
                name: ClassId(Symbol::from(*c)),
                is_module: false,
                parent: Some(ClassId(Symbol::from("ApplicationCable::Channel"))),
                includes: Vec::new(),
                methods: Vec::new(),
                nullable_columns: Vec::new(),
                origin: None,
                constants: Vec::new(),
                unknown_calls: Vec::new(),
            });
        }
        app.test_modules.push(tm);
        app
    }

    fn lowered(app: &App) -> String {
        crate::emit::ruby::emit_expr(&app.test_modules[0].tests[0].body)
    }

    #[test]
    fn subscribe_with_keywords_names_the_channel_and_strings_the_values() {
        let tm = test_module("PresenceChannelTest", CHANNEL_TEST_CASE, None, "subscribe room_id: room.id");
        let mut app = app_with(tm, &["PresenceChannel"]);
        let diags = apply_cable_test_case_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        let out = lowered(&app);
        assert!(out.contains(r#"subscribe_to("PresenceChannel", ["room_id"], [room.id.to_s])"#), "{out}");
    }

    #[test]
    fn a_bare_subscribe_and_the_tests_macro() {
        let tm = test_module(
            "RoomMessagesViaStockTurboChannelTest",
            CHANNEL_TEST_CASE,
            Some("Turbo::StreamsChannel"),
            "subscribe\n    assert_has_stream_for rooms(:hq)",
        );
        let mut app = app_with(tm, &[]);
        let diags = apply_cable_test_case_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        let out = lowered(&app);
        assert!(out.contains(r#"subscribe_to("Turbo::StreamsChannel", [], [])"#), "{out}");
        assert!(
            out.contains("assert_has_stream(Turbo::StreamsChannel.broadcasting_for(rooms(:hq)))"),
            "{out}"
        );
    }

    #[test]
    fn a_test_whose_channel_is_not_in_the_tree_is_reported_and_left() {
        let tm = test_module("GhostChannelTest", CHANNEL_TEST_CASE, None, "subscribe room_id: 1");
        let mut app = app_with(tm, &["PresenceChannel"]);
        let diags = apply_cable_test_case_lowering(&mut app);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(lowered(&app).contains("subscribe room_id: 1"), "{}", lowered(&app));
    }

    #[test]
    fn signed_stream_name_is_spelled_from_the_streamables_in_any_test() {
        let tm = test_module(
            "RoomTest",
            "ActiveSupport::TestCase",
            None,
            "a = Turbo::StreamsChannel.signed_stream_name([ @room, :messages ])\n    b = Turbo::StreamsChannel.signed_stream_name([ :rooms ])\n    c = Turbo::StreamsChannel.signed_stream_name([ rooms(:hq) ])",
        );
        let mut app = app_with(tm, &[]);
        let diags = apply_cable_test_case_lowering(&mut app);
        assert!(diags.is_empty(), "{diags:?}");
        let out = lowered(&app);
        assert!(out.contains(r##"Turbo::Streams::StreamName.signed("#{@room.to_gid_param}:messages")"##), "{out}");
        assert!(out.contains(r#"Turbo::Streams::StreamName.signed("rooms")"#), "{out}");
        assert!(out.contains(r#"Turbo::Streams::StreamName.signed(rooms(:hq).to_gid_param)"#), "{out}");
    }
}
