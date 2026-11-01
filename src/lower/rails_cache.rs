//! `Rails.cache.fetch(k, expires_in: t) { <String> }` → `fetch_str(k, t)`.
//!
//! The shared runtime's `Rails.cache` is two stores, not one (see
//! `runtime/ruby/rails.rb`): `fetch` recomputes, and `fetch_str` is a
//! typed String-keyed/String-valued store. A single store would have to
//! hold every value the block might produce — lobsters caches an Integer
//! count, a Tag list and rendered fragments through the same call — and
//! that box is exactly what the ruby-family runtime's typing bar keeps
//! out. So the compiler picks: a site whose block provably yields a
//! String routes to the store; everything else keeps recomputing, which
//! is what it did before this pass existed.
//!
//! Why it matters: `render_to_string` inside a `Rails.cache.fetch` is how
//! Rails apps cache a whole page fragment, and it is the shape that hurts
//! most when the cache is a no-op. Lobsters' `/u` renders a ~292KB invite
//! tree over every user and caches it for 24 hours; without the store it
//! rebuilt that tree on all 15 of its visits in the benchmark sequence.
//!
//! Two deliberate limits:
//!
//! - A block-pass site (`fetch(k, expires_in: 45, &block)` — lobsters'
//!   story and front-page caches) is left alone. The block belongs to the
//!   caller, so this pass cannot see what it yields.
//! - `expires_in:` is forwarded through `.to_i`, which reads seconds off
//!   an Integer literal and off an `ActiveSupport::Duration` alike. A
//!   site with no `expires_in:` gets `0` — never expires, matching
//!   Rails, where an entry without a TTL lives until eviction.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_rails_cache_lowering(app: &mut App) {
    if app_binds_cache_store(app) {
        return;
    }
    super::for_each_hook_body(app, &mut rewrite);
}

/// Is `Rails.cache` the app's own store? Then `fetch_str` (a method of
/// the runtime's store only) would be undefined on it: the pass stands
/// down and `fetch` runs as written. Only an emitted `Rails.cache =`
/// assignment rebinds it (a test swapping in a double): no target
/// installs `config.cache_store`, so whatever an app configures there,
/// `Rails.cache` stays the runtime's store, and standing down for it
/// would only turn its caching off. The assignment is the app's own
/// store when it names an `ActiveSupport::Cache::*` class the app
/// defines, or any value other than a Rails store class
/// (`Rails.cache = MyStore.new`).
pub(crate) fn app_binds_cache_store(app: &App) -> bool {
    let stores: Vec<&str> = app
        .library_classes
        .iter()
        .map(|c| &c.name)
        .chain(app.models.iter().map(|m| &m.name))
        .filter_map(|id| id.0.as_str().strip_prefix("ActiveSupport::Cache::"))
        .collect();
    let binds = |line: &str| {
        let line = line.trim();
        if line.starts_with('#') {
            return false;
        }
        let Some((lhs, rhs)) = line.split_once('=') else { return false };
        if rhs.starts_with('=') || rhs.starts_with('~') || lhs.ends_with(['!', '<', '>']) {
            return false;
        }
        if lhs.trim().trim_start_matches("::") != "Rails.cache" {
            return false;
        }
        let rhs = rhs.trim();
        let rails_store = rhs.trim_start_matches("::").starts_with("ActiveSupport::Cache::");
        !rails_store || stores.iter().any(|store| rhs.contains(store))
    };
    app.sources.iter().any(|source| rails_cache_assignments(&source.text).iter().any(|s| binds(s)))
}

/// Each statement of `text` that mentions `Rails.cache`, as one line: a
/// line ending in `=`, `,`, `(` or `\` continues onto the next, so
/// `Rails.cache =` with its value below it is whole.
fn rails_cache_assignments(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(trimmed.trim_end_matches('\\'));
        if !trimmed.starts_with('#') && trimmed.ends_with(['=', ',', '(', '\\']) {
            continue;
        }
        let statement = std::mem::take(&mut current);
        if statement.contains("Rails.cache") {
            out.push(statement);
        }
    }
    if current.contains("Rails.cache") {
        out.push(current);
    }
    out
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    rewrite_node(expr);
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(r), method, args, block: Some(b), .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "fetch" || args.is_empty() || args.len() > 2 || !is_rails_cache(r) {
        return;
    }
    if !yields_string(b) {
        return;
    }

    let ttl = args.get(1).and_then(expires_in).map(to_i).unwrap_or_else(|| {
        Expr::new(expr.span, ExprNode::Lit { value: Literal::Int { value: 0 } })
    });

    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, args, block, .. } = node else { unreachable!() };
    let key = args.into_iter().next().unwrap();
    *expr.node = ExprNode::Send {
        recv,
        method: Symbol::from("fetch_str"),
        args: vec![key, ttl],
        block,
        parenthesized: true,
    };
    let _ = span;
}

/// `Rails.cache` — the receiver the store hangs off. An app object that
/// happens to answer `fetch` is not it.
pub(crate) fn is_rails_cache(e: &Expr) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node else { return false };
    if method.as_str() != "cache" || !args.is_empty() {
        return false;
    }
    matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Rails")
}

/// The last expression a block body evaluates to, looking through the
/// sequence and let nesting the body is built from.
fn tail(e: &Expr) -> &Expr {
    match &*e.node {
        ExprNode::Seq { exprs } => exprs.last().map(tail).unwrap_or(e),
        ExprNode::Let { body, .. } => tail(body),
        _ => e,
    }
}

/// Does this block provably yield a String? Either the type walker says
/// so, or the tail is `render_to_string`, whose contract is a String and
/// which this pass runs ahead of the render lowering to catch.
fn yields_string(block: &Expr) -> bool {
    let ExprNode::Lambda { body, .. } = &*block.node else { return false };
    let t = tail(body);
    if matches!(t.ty, Some(Ty::Str)) {
        return true;
    }
    matches!(&*t.node, ExprNode::Send { method, .. } if method.as_str() == "render_to_string")
}

/// The `expires_in:` value out of the options hash, whichever spelling
/// the source used (`expires_in: 45` or `:expires_in => 45`).
fn expires_in(opts: &Expr) -> Option<Expr> {
    let ExprNode::Hash { entries, .. } = &*opts.node else { return None };
    entries
        .iter()
        .find(|(k, _)| {
            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                if value.as_str() == "expires_in")
        })
        .map(|(_, v)| v.clone())
}

/// `<ttl>.to_i` — seconds off an Integer or an ActiveSupport::Duration.
fn to_i(v: Expr) -> Expr {
    let span = v.span;
    let mut call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(v),
            method: Symbol::from("to_i"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    );
    // Typed here: this pass runs after analysis, and an untyped Send on
    // a typed receiver reads at the post-lowering diagnose as a failed
    // dispatch (`no known method `to_i` on Integer`).
    call.ty = Some(crate::ty::Ty::Int);
    call
}
