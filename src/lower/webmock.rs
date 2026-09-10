//! WebMock, lowered to a typed stub table.
//!
//! `WebMock.stub_request(:get, url).to_return(status:, body:, headers:)`
//! installs an interception in a registry that WebMock's monkeypatch of
//! `Net::HTTP` consults — a Ruby metaprogramming library with nothing
//! for a strict target to compile against, so campfire's forty stub
//! sites were a permanent ceiling for the same reason mocha's were.
//!
//! And they are not, for the same reason: the set of stub sites is
//! CLOSED at transpile time, and every one of them matches on nothing
//! but `(verb, url)`. Measured across campfire's suite before designing
//! anything — 40 `stub_request` calls (25 `:get`, 8 `:head`, 7 `:post`),
//! ONE `.with(...)` request matcher (`webhook_test`'s `hash_including`,
//! which is mocha's and stays ceiling), and `to_return` carrying only
//! `status:`, `body:` and `headers:`. So the lowering is a rewrite to
//! one call:
//!
//! ```text
//!   WebMock.stub_request(:get, url).to_return(status: 302, headers: { location: l })
//!     -> HttpStub.stub("GET", url, 302, "", { "location" => l })
//! ```
//!
//! `HttpStub` is a runtime slot, and — as with `lower::mocha` — the
//! rewrite runs on EVERY target because a lowering may not branch on
//! the target. The FILE differs: the strict targets get the table
//! (`runtime/spinel/http_stub.rb`) and a reopened `Net::HTTP` that
//! consults it before any socket opens (`runtime/spinel/net_http.rb`);
//! the ruby family gets a three-method delegate onto the real WebMock
//! gem (`project::HTTP_STUB_WEBMOCK_DELEGATE`), so today's CRuby lane
//! keeps exercising the gem it always did.
//!
//! HEADERS ARE NORMALISED HERE, not at run time. campfire writes both
//! spellings — `content_type:` and `"Content-Type" =>` — and the
//! response class the strict double builds stores names lower-cased and
//! dash-spelled. Doing it in the lowering means the table's element type
//! is `Hash[String, String]` by construction, which is what lets spinel
//! infer it; a value that is not a String literal is wrapped in `.to_s`
//! for the same reason (`content_length: 1.gigabyte`).
//!
//! WHAT IS DELIBERATELY LEFT ALONE. A chain this pass does not fully
//! understand keeps its WebMock spelling, so it fails loudly at the
//! `WebMock` constant on a strict target rather than being silently
//! dropped — the failure mode that matters is a stub that never took
//! while the test reports green. That covers `.with(...)` matchers, a
//! `stub_request` with no `to_return`, a `status:` given as
//! `[code, message]`, a non-literal `headers:`, and `:any` as the verb.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// The runtime file that defines `HttpStub`. The emitted helper
/// REQUIRES it so the clear call below is unconditional — see
/// `lower::mocha::stub_requires` for what guarding on `defined?` cost.
const REQUIRE: &str = "../runtime/http_stub";

/// The verbs WebMock accepts as a Symbol, and the wire spelling each
/// becomes. `:any` is absent on purpose: the table keys on one verb.
const VERBS: &[(&str, &str)] = &[
    ("get", "GET"),
    ("post", "POST"),
    ("put", "PUT"),
    ("patch", "PATCH"),
    ("delete", "DELETE"),
    ("head", "HEAD"),
    ("options", "OPTIONS"),
];

/// `require_relative` line for the runtime file that defines the slot.
pub fn stub_requires() -> String {
    format!("require_relative \"{REQUIRE}\"\n")
}

/// The clear call, for the helper to run between tests. A stub that
/// outlives its test answers for every later one in the file — the leak
/// WebMock's own `after_teardown` exists to prevent.
pub fn stub_clear_lines(indent: &str) -> String {
    format!("{indent}HttpStub.clear\n")
}

pub fn apply_webmock_lowering(app: &mut App) {
    for tm in &mut app.test_modules {
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
}

/// `WebMock` / `::WebMock` as a receiver.
fn is_webmock(recv: &Expr) -> bool {
    let ExprNode::Const { path } = &*recv.node else { return false };
    path.last().is_some_and(|s| s.as_str() == "WebMock")
}

fn str_lit(span: crate::span::Span, s: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } })
}

fn http_stub_call(span: crate::span::Span, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("HttpStub")] })),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

/// Peels outside-in, as `mocha::rewrite` does: `to_return` is the
/// outermost send and `stub_request` its receiver.
fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);

    let ExprNode::Send { recv: Some(outer), method, args, block: None, .. } = &*expr.node else {
        return;
    };
    let span = expr.span;
    let replacement = match method.as_str() {
        "to_return" => lower_to_return(outer, args, span),
        "disable_net_connect!" if is_webmock(outer) => lower_disable_net_connect(args, span),
        "reset!" if is_webmock(outer) && args.is_empty() => Some(http_stub_call(span, "clear", vec![])),
        _ => None,
    };
    if let Some(r) = replacement {
        *expr = r;
    }
}

/// `WebMock.stub_request(:verb, url)` -> `("VERB", url)`.
fn stub_request_head(head: &Expr) -> Option<(&'static str, Expr)> {
    let ExprNode::Send { recv: Some(w), method, args, block: None, .. } = &*head.node else {
        return None;
    };
    if !is_webmock(w) || method.as_str() != "stub_request" || args.len() != 2 {
        return None;
    }
    let ExprNode::Lit { value: Literal::Sym { value: verb } } = &*args[0].node else { return None };
    let wire = VERBS.iter().find(|(s, _)| *s == verb.as_str()).map(|(_, w)| *w)?;
    Some((wire, args[1].clone()))
}

/// The one keyword hash `to_return` takes, as `(key, value)` pairs with
/// every key a literal Symbol — or None, which leaves the chain alone.
fn literal_sym_entries(args: &[Expr]) -> Option<Vec<(String, Expr)>> {
    if args.len() != 1 {
        return None;
    }
    let ExprNode::Hash { entries, .. } = &*args[0].node else { return None };
    entries
        .iter()
        .map(|(k, v)| {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
            Some((key.as_str().to_string(), v.clone()))
        })
        .collect()
}

fn lower_to_return(head: &Expr, args: &[Expr], span: crate::span::Span) -> Option<Expr> {
    let (verb, url) = stub_request_head(head)?;
    let entries = literal_sym_entries(args)?;
    if entries.iter().any(|(k, _)| !matches!(k.as_str(), "status" | "body" | "headers")) {
        return None;
    }
    let find = |name: &str| entries.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());

    // `status: [200, "OK"]` is WebMock's two-element form; the table
    // stores one Integer, so that spelling stays WebMock's.
    let status = match find("status") {
        Some(s) if matches!(&*s.node, ExprNode::Array { .. }) => return None,
        Some(s) => s,
        None => Expr::new(span, ExprNode::Lit { value: Literal::Int { value: 200 } }),
    };

    // WebMock reads an IO or Pathname body for the caller. The corpus
    // hands it exactly one non-String: `file_fixture("moon.jpg")`, a
    // Pathname on every target, so the read is spelled at the site.
    let body = match find("body") {
        Some(b) if is_bare_call(&b, "file_fixture") => Expr::new(
            b.span,
            ExprNode::Send {
                recv: Some(b),
                method: Symbol::from("binread"),
                args: vec![],
                block: None,
                parenthesized: true,
            },
        ),
        Some(b) => b,
        None => str_lit(span, ""),
    };

    let headers = match find("headers") {
        Some(h) => normalised_headers(&h)?,
        None => Expr::new(span, ExprNode::Hash { entries: vec![], kwargs: false }),
    };

    Some(http_stub_call(span, "stub", vec![str_lit(span, verb), url, status, body, headers]))
}

fn is_bare_call(e: &Expr, name: &str) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == name)
}

/// `{ content_type: "text/html", "Content-Length" => 1.gigabyte }`
/// -> `{ "content-type" => "text/html", "content-length" => 1.gigabyte.to_s }`.
///
/// Every key must be a literal Symbol or String; a computed key leaves
/// the whole chain alone.
fn normalised_headers(h: &Expr) -> Option<Expr> {
    let ExprNode::Hash { entries, .. } = &*h.node else { return None };
    let mut out = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        let name = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => return None,
        };
        let name = name.to_ascii_lowercase().replace('_', "-");
        let value = match &*v.node {
            ExprNode::Lit { value: Literal::Str { .. } } | ExprNode::StringInterp { .. } => v.clone(),
            ExprNode::Send { method, args, .. } if method.as_str() == "to_s" && args.is_empty() => v.clone(),
            _ => Expr::new(
                v.span,
                ExprNode::Send {
                    recv: Some(v.clone()),
                    method: Symbol::from("to_s"),
                    args: vec![],
                    block: None,
                    parenthesized: true,
                },
            ),
        };
        out.push((str_lit(k.span, &name), value));
    }
    Some(Expr::new(h.span, ExprNode::Hash { entries: out, kwargs: false }))
}

/// `WebMock.disable_net_connect!` / `WebMock.disable_net_connect!(allow: hosts)`
/// -> `HttpStub.allow_net_connect(hosts)`. The strict double has no
/// transport to allow; the ruby family's delegate hands it to WebMock.
fn lower_disable_net_connect(args: &[Expr], span: crate::span::Span) -> Option<Expr> {
    let hosts = if args.is_empty() {
        Expr::new(span, ExprNode::Array { elements: vec![], style: Default::default() })
    } else {
        let entries = literal_sym_entries(args)?;
        let [(key, value)] = entries.as_slice() else { return None };
        if key != "allow" {
            return None;
        }
        value.clone()
    };
    Some(http_stub_call(span, "allow_net_connect", vec![hosts]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn sp() -> Span {
        Span::synthetic()
    }
    fn sym(s: &str) -> Expr {
        Expr::new(sp(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
    }
    fn int(v: i64) -> Expr {
        Expr::new(sp(), ExprNode::Lit { value: Literal::Int { value: v } })
    }
    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            sp(),
            ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true },
        )
    }
    fn webmock() -> Expr {
        Expr::new(sp(), ExprNode::Const { path: vec![Symbol::from("WebMock")] })
    }
    fn kwargs(entries: Vec<(Expr, Expr)>) -> Expr {
        Expr::new(sp(), ExprNode::Hash { entries, kwargs: true })
    }
    fn stub_request(verb: &str) -> Expr {
        send(Some(webmock()), "stub_request", vec![sym(verb), str_lit(sp(), "https://www.example.com/")])
    }
    fn str_of(e: &Expr) -> &str {
        match &*e.node {
            ExprNode::Lit { value: Literal::Str { value } } => value,
            other => panic!("expected a String literal, got {other:?}"),
        }
    }

    #[test]
    fn rewrites_to_return_with_normalised_headers() {
        // stub_request(:get, u).to_return(status: 302, headers: { location: "l", "Content-Length" => 1.gigabyte })
        let headers = Expr::new(
            sp(),
            ExprNode::Hash {
                entries: vec![
                    (sym("location"), str_lit(sp(), "l")),
                    (str_lit(sp(), "Content-Length"), send(Some(int(1)), "gigabyte", vec![])),
                ],
                kwargs: false,
            },
        );
        let mut e = send(
            Some(stub_request("get")),
            "to_return",
            vec![kwargs(vec![(sym("status"), int(302)), (sym("headers"), headers)])],
        );
        rewrite(&mut e);

        let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node else { panic!("{:?}", e.node) };
        assert!(matches!(&*recv.node, ExprNode::Const { path } if path[0].as_str() == "HttpStub"));
        assert_eq!(method.as_str(), "stub");
        assert_eq!(args.len(), 5);
        assert_eq!(str_of(&args[0]), "GET");
        assert_eq!(str_of(&args[1]), "https://www.example.com/");
        assert!(matches!(&*args[2].node, ExprNode::Lit { value: Literal::Int { value: 302 } }));
        assert_eq!(str_of(&args[3]), "", "a missing body is the empty String");
        let ExprNode::Hash { entries, kwargs: false } = &*args[4].node else { panic!("{:?}", args[4].node) };
        assert_eq!(str_of(&entries[0].0), "location");
        assert_eq!(str_of(&entries[1].0), "content-length");
        // The non-String value is spelled `.to_s`.
        assert!(matches!(&*entries[1].1.node, ExprNode::Send { method, .. } if method.as_str() == "to_s"));
        assert!(matches!(&*entries[0].1.node, ExprNode::Lit { value: Literal::Str { .. } }));
    }

    #[test]
    fn defaults_when_only_status_is_given() {
        let mut e = send(Some(stub_request("post")), "to_return", vec![kwargs(vec![(sym("status"), int(200))])]);
        rewrite(&mut e);
        let ExprNode::Send { method, args, .. } = &*e.node else { panic!() };
        assert_eq!(method.as_str(), "stub");
        assert_eq!(str_of(&args[0]), "POST");
        assert_eq!(str_of(&args[3]), "");
        assert!(matches!(&*args[4].node, ExprNode::Hash { entries, .. } if entries.is_empty()));
    }

    #[test]
    fn reads_a_file_fixture_body() {
        let body = send(None, "file_fixture", vec![str_lit(sp(), "moon.jpg")]);
        let mut e = send(Some(stub_request("post")), "to_return", vec![kwargs(vec![(sym("body"), body)])]);
        rewrite(&mut e);
        let ExprNode::Send { args, .. } = &*e.node else { panic!() };
        assert!(matches!(&*args[3].node, ExprNode::Send { method, .. } if method.as_str() == "binread"));
    }

    /// A chain the pass does not understand keeps its WebMock spelling:
    /// `.with(...)` matchers, the two-element status, `:any`.
    #[test]
    fn leaves_what_it_does_not_understand_alone() {
        let with = send(Some(stub_request("post")), "with", vec![kwargs(vec![(sym("body"), str_lit(sp(), "x"))])]);
        let mut e = send(Some(with), "to_return", vec![kwargs(vec![(sym("status"), int(200))])]);
        rewrite(&mut e);
        assert!(matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "to_return"));

        let array = Expr::new(sp(), ExprNode::Array { elements: vec![int(200), str_lit(sp(), "OK")], style: Default::default() });
        let mut e = send(Some(stub_request("get")), "to_return", vec![kwargs(vec![(sym("status"), array)])]);
        rewrite(&mut e);
        assert!(matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "to_return"));

        let mut e = send(Some(stub_request("any")), "to_return", vec![kwargs(vec![(sym("status"), int(200))])]);
        rewrite(&mut e);
        assert!(matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "to_return"));
    }

    #[test]
    fn lowers_disable_net_connect_and_reset() {
        let hosts = Expr::new(sp(), ExprNode::Array { elements: vec![str_lit(sp(), "h")], style: Default::default() });
        let mut e = send(Some(webmock()), "disable_net_connect!", vec![kwargs(vec![(sym("allow"), hosts)])]);
        rewrite(&mut e);
        let ExprNode::Send { method, args, .. } = &*e.node else { panic!() };
        assert_eq!(method.as_str(), "allow_net_connect");
        assert!(matches!(&*args[0].node, ExprNode::Array { elements, .. } if elements.len() == 1));

        let mut e = send(Some(webmock()), "reset!", vec![]);
        rewrite(&mut e);
        assert!(matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "clear"));
    }
}
