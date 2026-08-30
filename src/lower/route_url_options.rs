//! `x_path(…, host: "example.com")` → `x_path(…)`.
//!
//! Rails partitions a route helper's option hash into the keys
//! `url_for` consumes ITSELF and the keys it forwards to the path
//! generator. `ActionDispatch::Routing::RouteSet::RESERVED_OPTIONS`
//! (actionpack 8.1.3, `route_set.rb:838`) is that first list:
//!
//! ```text
//! [:host, :protocol, :port, :subdomain, :domain, :tld_length,
//!  :trailing_slash, :anchor, :params, :only_path, :script_name,
//!  :original_script_name]
//! ```
//!
//! `url_for` deletes every one of them from `path_options` before
//! generating (`route_set.rb:864`), so none can ever reach the query
//! string. We forwarded them all, and a leftover option is a query key
//! here: campfire's `room_at_message_url(@room, msg, host:
//! "once.campfire.test")` rendered `/rooms/1/@5?host=once.campfire.test`
//! where Rails renders `http://once.campfire.test/rooms/1/@5`. That is
//! the app's own broadcast assertion failing, and note it is wrong
//! TWICE — a query key that should not be there, and an authority that
//! should. Dropping `host:` alone fixes only the first half and leaves
//! a bare path being compared against an absolute URL.
//!
//! **A rule table, not a special case for `host:`.** The list splits
//! three ways once you ask what each key does to a PATH:
//!
//!   * [`HOST_ONLY_OPTIONS`] — the seven that only describe the host
//!     half of a URL. `path_for` renders no host at all, so dropping
//!     them is EXACT rather than an approximation. This pass strips
//!     them from the call site; the demand survey in
//!     `routes_to_library` refuses to make a parameter out of them.
//!
//!     **On the `_url` spelling they are not dropped — they are the
//!     answer.** `x_url(…, host: h)` renders `"http://#{h}#{x_path(…)}"`,
//!     the same shape the view lowerer grounds a hostless `_url` with
//!     (`Rails.application.domain` in place of `h`) and the same one
//!     `emit::ruby::library::rewrite_url_helpers_absolute` builds for
//!     the explicit `…routes.url_helpers.x_url(…, host:)` chain.
//!     Dropping the host here would be the second half of the campfire
//!     bug rather than its fix: the button that assertion compares
//!     against holds an ABSOLUTE URL.
//!   * `anchor:` — the fragment. It DOES belong on a path, and
//!     `routes_to_library` now renders it (`#tag`, after the query
//!     string, exactly where `path_for` puts it). Four lobsters call
//!     sites and two writebook ones were passing it to a helper that
//!     had no such parameter.
//!   * `format:` — owned by [`super::route_format_suffix`], which
//!     monomorphizes the helper instead of widening its signature.
//!
//! The four left over — `script_name:`, `original_script_name:`,
//! `trailing_slash:` and `params:` — each genuinely change the path,
//! and none is modeled. They stay query keys, which is visibly wrong
//! rather than silently wrong, and no corpus app writes one on an app
//! route. Ledgered in `docs/pipeline/runtime.md`.
//!
//! Scope, and ordering: bare `*_path` / `*_url` calls whose name
//! matches a route in the app's own table, rewritten before
//! `lower_routes_to_library_functions` surveys those same call sites.
//! `rails_blob_path(…, only_path: true)` is an ActiveStorage helper
//! with its own declared parameter and names no app route, so it is
//! untouched.

use crate::app::App;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;

/// The `RESERVED_OPTIONS` entries that describe only the HOST half of
/// a URL — protocol, authority, and the choice of whether to render
/// one at all.
///
/// `path_for` is `url_for(…, PATH, …)`, and the `PATH` strategy
/// returns `options[:path]` with the script name, params and anchor
/// applied and nothing else (`http/url.rb`). Every key here feeds
/// `build_host_url`, which that strategy never calls. So a `_path`
/// helper drops them, and drops them exactly.
pub(crate) const HOST_ONLY_OPTIONS: &[&str] = &[
    "host",
    "protocol",
    "port",
    "subdomain",
    "domain",
    "tld_length",
    "only_path",
];

pub fn apply_route_url_options_lowering(app: &mut App) {
    let helpers = super::route_format_suffix::route_helper_names(app);
    if helpers.is_empty() {
        return;
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &helpers));
    for view in &mut app.views {
        rewrite(&mut view.body, &helpers);
    }
    // `for_each_hook_body` does not reach test bodies, and the call
    // site this pass exists for is one — campfire's
    // `MessagesControllerTest`.
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &helpers);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &helpers);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &helpers);
        }
    }
}

fn rewrite(expr: &mut Expr, helpers: &std::collections::HashSet<String>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, helpers));
    let Some((stem, host, protocol)) = strip_host_options(expr, helpers) else {
        return;
    };
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { args, parenthesized, .. } = node else { unreachable!() };
    // A BARE `<stem>_path` send, not a qualified one: the demand survey
    // reads these call sites for query keys and the `RouteHelpers.`
    // receiver goes on afterwards, so qualifying here would hide the
    // call from both.
    let path_call = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from(format!("{stem}_path")),
            args,
            block: None,
            parenthesized,
        },
    );
    // `protocol:` rides bare (`"https"`), the convention
    // `rewrite_url_helpers_absolute` already set — Rails'
    // `normalize_protocol` accepts `"https"` and `"https://"` alike and
    // we accept only the first.
    let mut parts: Vec<InterpPart> = Vec::new();
    match protocol {
        Some(p) => parts.push(InterpPart::Expr { expr: p }),
        None => parts.push(InterpPart::Text { value: "http".to_string() }),
    }
    parts.push(InterpPart::Text { value: "://".to_string() });
    parts.push(InterpPart::Expr { expr: host });
    parts.push(InterpPart::Expr { expr: path_call });
    *expr.node = ExprNode::StringInterp { parts };
    expr.ty = Some(crate::ty::Ty::Str);
}

/// Remove every [`HOST_ONLY_OPTIONS`] key from one route-helper call's
/// trailing kwargs hash.
///
/// Answers `Some((stem, host, protocol))` for the one case where the
/// removal is not the whole story: a `_url` spelling that named a
/// `host:` and did not ask for `only_path`. Its caller rebuilds that
/// into `"<protocol>://<host><stem>_path(…)"`. Every other call —
/// `_path`, a hostless `_url`, `x_url(only_path: true)` — is finished
/// by the strip alone and answers None.
fn strip_host_options(
    expr: &mut Expr,
    helpers: &std::collections::HashSet<String>,
) -> Option<(String, Expr, Option<Expr>)> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &mut *expr.node else {
        return None;
    };
    if !helpers.contains(method.as_str()) {
        return None;
    }
    let last = args.last_mut()?;
    let ExprNode::Hash { entries, kwargs: true } = &mut *last.node else {
        return None;
    };
    // Read the two that BUILD a host before dropping them: on the `_url`
    // spelling they are not dropped at all — they ARE the host half, and
    // this is the last place that knows the caller named one.
    let mut host: Option<Expr> = None;
    let mut protocol: Option<Expr> = None;
    let mut only_path = false;
    for (k, v) in entries.iter() {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
            continue;
        };
        match value.as_str() {
            "host" => host = Some(v.clone()),
            "protocol" => protocol = Some(v.clone()),
            "only_path" => {
                only_path =
                    matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } });
            }
            _ => {}
        }
    }
    entries.retain(|(k, _)| {
        !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if HOST_ONLY_OPTIONS.contains(&value.as_str()))
    });
    // A hash that held ONLY host options is gone entirely, so the call
    // reaches a helper that grew no parameter for them; one that also
    // carried query keys keeps them. The same disposal
    // `route_format_suffix` makes, for the same reason.
    if entries.is_empty() {
        args.pop();
    }
    // `_path` is done: the seven describe a host it does not render.
    let stem = method.as_str().strip_suffix("_url")?;
    // `x_url(…, only_path: true)` is Rails asking the URL spelling for
    // a path, which is the `_path` helper — the strip above already
    // made it one.
    if only_path {
        return None;
    }
    // `x_url(…, host: h)` names the host EXPLICITLY, and dropping it
    // would be the second half of the campfire bug rather than its fix:
    // the copy-link button that assertion compares against holds an
    // ABSOLUTE URL. The view lowerer already grounds a hostless `_url`
    // as `"http://#{Rails.application.domain}#{…_path}"`; this is that
    // shape with the caller's host in place of the default, and it is
    // what `emit::ruby::library::rewrite_url_helpers_absolute` builds
    // for the explicit `…routes.url_helpers.x_url(…, host:)` chain.
    // Two spellings, one rendering.
    Some((stem.to_string(), host?, protocol))
}
