//! `Rails.application.config.<key>` → `Rails.application.<key>`.
//!
//! Rails' config object takes arbitrary keys, so an app-defined one has
//! no structure to model: `config.app_version = …` in an initializer IS
//! the definition, and ingest lifts each assignment to a reader on the
//! `Rails::Application` reopen (see `extract_config_assignments`). This
//! is the other half — the reads, rewritten to call that reader.
//!
//! Dropping the `config` hop rather than synthesizing a config object
//! keeps one shape for every application-level value the app asks for:
//! a lifted config key now reads exactly like `Rails.application.domain`
//! (lobsters' `class << Rails.application` idiom), which the runtime and
//! every emitter already handle.
//!
//! Only rewrites keys the lift actually produced. A read of a framework
//! key — `Rails.application.config.eager_load` — is left alone to fail
//! visibly rather than silently answering nil from a reader nobody
//! defined.
//!
//! ## Writes, and the `tap` a test writes through
//!
//! Ingest lifts a WRITER beside every reader (`<key>=`), because a
//! test can assign a config key the way the initializer did: campfire's
//! `test/test_helper.rb` replaces the web-push pool before every test,
//!
//! ```ruby
//! Rails.configuration.tap do |config|
//!   config.x.web_push_pool.shutdown
//!   config.x.web_push_pool = WebPush::Pool.new(…)
//! end
//! ```
//!
//! and both halves of that are handled here: the `tap` is unwrapped
//! (its parameter IS the receiver, so every use of `config` becomes
//! `Rails.configuration` and the block body runs in place), and an
//! assignment whose target chain peels to a lifted key becomes a call
//! on the writer. TEST BODIES ARE WALKED for the same reason — the
//! reads above were only ever app-side until a test asked the
//! configuration for the pool it was waiting on.
//!
//! ## Framework config the emit fixes by construction
//!
//! One framework read is different, and gets grounded to its VALUE
//! rather than left to fail: `ActionCable.server.config.mount_path`. The
//! "fail visibly" rule above is for keys whose value we do not know — but
//! this one we do, because we choose it: every lane mounts the cable
//! endpoint at `/cable` (`runtime/spinel/scaffold/main.rb`,
//! `runtime/rust/server.rs`), and it is not configurable. Campfire's
//! cable helper joins it with `request.script_name` to build the socket
//! URL, so it is read on every page that renders the layout.
//!
//! Grounding it to the literal beats shipping an `ActionCable.server.
//! config` object chain: three objects deep, existing only to answer one
//! constant, and the kind of dynamic surface a strict target cannot
//! resolve.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};

/// Where every lane serves the Action Cable WebSocket.
const CABLE_MOUNT_PATH: &str = "/cable";

pub fn apply_config_reader_lowering(app: &mut App) {
    ground_cable_mount_path(app);
    // Reader name -> the type of the value it answers, from the lifted
    // method's own body. Stamped onto each rewritten read for the same
    // reason the synthesized `application` hop below is stamped: this
    // node is BORN after the analyzer has run, so nothing else will ever
    // type it, and a later pass that grounds by receiver type would see
    // nothing where the source plainly names a Hash.
    // `Analyzer::type_rails_application_body` is what puts a type on
    // those bodies at all.
    //
    // A memoized leaf reader's own body ends in a slot read, which the
    // analyzer cannot type; the expression it memoizes lives in the
    // `<key>__build` sibling ingest lifted beside it, so that is where
    // the leaf's type is read from.
    let lifted: Vec<(crate::ident::Symbol, Option<crate::ty::Ty>)> = match &app.rails_application {
        Some(lc) => lc
            .methods
            .iter()
            .filter(|m| !m.name.as_str().ends_with("__build") && !m.name.as_str().ends_with('='))
            .map(|m| {
                let build = format!("{}__build", m.name.as_str());
                let ty = lc
                    .methods
                    .iter()
                    .find(|b| b.name.as_str() == build)
                    .map_or_else(|| body_ty(&m.body), |b| body_ty(&b.body));
                (m.name.clone(), ty)
            })
            .collect(),
        None => return,
    };
    if lifted.is_empty() {
        return;
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &lifted));
    super::for_each_test_body(app, &mut |e| rewrite(e, &lifted));
    let lifted_for_views = lifted.clone();
    for view in &mut app.views {
        rewrite(&mut view.body, &lifted_for_views);
    }
}

/// The type a reader answers: its body's, or its last statement's when
/// the body is a sequence.
fn body_ty(body: &Expr) -> Option<crate::ty::Ty> {
    let last = match &*body.node {
        ExprNode::Seq { exprs } => exprs.last()?,
        _ => body,
    };
    last.ty.clone().filter(|t| !matches!(t, crate::ty::Ty::Var { .. } | crate::ty::Ty::Untyped))
}

fn ground_cable_mount_path(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite_cable_mount_path);
    for view in &mut app.views {
        rewrite_cable_mount_path(&mut view.body);
    }
}

/// `ActionCable.server.config.mount_path` → `"/cable"`.
fn rewrite_cable_mount_path(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_cable_mount_path);
    if !is_cable_mount_path(expr) {
        return;
    }
    let span = expr.span;
    let mut lit = Expr::new(
        span,
        ExprNode::Lit { value: Literal::Str { value: CABLE_MOUNT_PATH.to_string() } },
    );
    lit.ty = Some(crate::ty::Ty::Str);
    *expr = lit;
}

/// The exact `ActionCable.server.config.mount_path` chain — walked hop
/// by hop so no shorter or differently-rooted chain matches.
fn is_cable_mount_path(expr: &Expr) -> bool {
    let mut cur = &*expr.node;
    for step in ["mount_path", "config", "server"] {
        let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = cur else {
            return false;
        };
        if method.as_str() != step || !args.is_empty() {
            return false;
        }
        cur = &r.node;
    }
    matches!(cur, ExprNode::Const { path }
        if path.last().is_some_and(|s| s.as_str() == "ActionCable"))
}

/// THE LONGEST CHAIN WINS, so this walks OUTERMOST-FIRST.
///
/// `config.x.vapid.public_key` contains `config.x.vapid`, and since the
/// group lift both names are now lifted readers. Rewriting children
/// first replaced the INNER one and left `Rails.application.x_vapid
/// .public_key` — `undefined method 'public_key' for an instance of
/// Hash`, on every page whose layout reads the VAPID key. Peeling the
/// whole node before descending takes the longest key the chain spells,
/// which is the one the reader was lifted from.
///
/// A node that does not peel still recurses, so a chain nested inside
/// an argument or a block is reached exactly as before; a node that
/// DOES peel has nothing left to walk but its `Rails` anchor.
fn rewrite(expr: &mut Expr, lifted: &[(crate::ident::Symbol, Option<crate::ty::Ty>)]) {
    unwrap_config_tap(expr);
    if rewrite_write_here(expr, lifted) {
        return;
    }
    if !rewrite_here(expr, lifted) {
        expr.node.for_each_child_mut(&mut |c| rewrite(c, lifted));
    }
}

/// `Rails.configuration.tap do |config| … end` → the block body in
/// place, every `config` read as `Rails.configuration`. `tap` yields
/// its receiver and answers it, so the block's parameter is a second
/// name for the receiver and nothing else; substituting the receiver
/// for the name is the whole meaning. The body's statements then peel
/// like any other config chain.
fn unwrap_config_tap(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(r), method, args, block: Some(b), .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "tap" || !args.is_empty() || !is_config_root(r) {
        return;
    }
    let ExprNode::Lambda { params, body, rest_param: None, block_param: None, .. } = &*b.node else {
        return;
    };
    let [param] = params.as_slice() else { return };
    let receiver = r.clone();
    let mut body = body.clone();
    substitute_var(&mut body, param, &receiver);
    let span = expr.span;
    let exprs = match *body.node {
        ExprNode::Seq { exprs } => exprs,
        _ => vec![body],
    };
    // The statements alone. `tap` answers its receiver, but there is
    // no config OBJECT in the emit to answer with — every key is a
    // reader on `Rails.application` — and a bare `Rails.configuration`
    // left in the sequence is a NameError. A config `tap` is written
    // for its statements; its value is the one thing not modeled.
    *expr = Expr::new(span, ExprNode::Seq { exprs });
}

/// `Rails.configuration` / `Rails.application.config` — the two
/// spellings of the object a config chain hangs off.
fn is_config_root(expr: &Expr) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    match method.as_str() {
        "configuration" => is_rails_const(r),
        "config" => matches!(&*r.node, ExprNode::Send { recv: Some(rr), method, args, block: None, .. }
            if method.as_str() == "application" && args.is_empty() && is_rails_const(rr)),
        _ => false,
    }
}

fn is_rails_const(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Rails")
}

/// Replace every read of local `name` in `expr` with `with`.
fn substitute_var(expr: &mut Expr, name: &crate::ident::Symbol, with: &Expr) {
    if let ExprNode::Var { name: n, .. } = &*expr.node {
        if n == name {
            let span = expr.span;
            let mut replacement = with.clone();
            replacement.span = span;
            *expr = replacement;
            return;
        }
    }
    expr.node.for_each_child_mut(&mut |c| substitute_var(c, name, with));
}

/// `Rails.configuration.x.web_push_pool = value` →
/// `Rails.application.x_web_push_pool=(value)`, when the target chain
/// peels to a lifted key. Descends into the value first: it is app code
/// and may itself read the key being replaced (campfire's does —
/// `WebPush::Pool.new(invalid_subscription_handler: config.x
/// .web_push_pool.invalid_subscription_handler)`).
fn rewrite_write_here(
    expr: &mut Expr,
    lifted: &[(crate::ident::Symbol, Option<crate::ty::Ty>)],
) -> bool {
    // Ingest spells `a.b = v` as a Send of `b=`; the `Attr` LValue is
    // the op-assign forms' spelling. Both are the same write.
    let (recv, name, value) = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if args.len() == 1 && method.as_str().ends_with('=') && !method.as_str().ends_with("==") =>
        {
            (r, method.as_str().trim_end_matches('=').to_string(), &args[0])
        }
        ExprNode::Assign { target: crate::expr::LValue::Attr { recv, name }, value } => {
            (recv, name.as_str().to_string(), value)
        }
        _ => return false,
    };
    let Some((application, mut segments)) = peel_config_chain(recv) else {
        return false;
    };
    segments.push(name);
    let key = crate::ident::Symbol::from(segments.join("_"));
    if !lifted.iter().any(|(n, _)| n == &key) {
        return false;
    }
    let mut value = value.clone();
    rewrite(&mut value, lifted);
    let span = expr.span;
    let ty = value.ty.clone();
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(application),
            method: crate::ident::Symbol::from(format!("{}=", key.as_str())),
            args: vec![value],
            block: None,
            parenthesized: true,
        },
    );
    expr.ty = ty;
    true
}

/// Rewrite this node if it is a lifted config read; `false` when it is
/// not, and the caller then descends.
fn rewrite_here(
    expr: &mut Expr,
    lifted: &[(crate::ident::Symbol, Option<crate::ty::Ty>)],
) -> bool {
    let Some((application, segments)) = peel_config_chain(expr) else {
        return false;
    };
    let key = crate::ident::Symbol::from(segments.join("_"));
    let Some((_, reader_ty)) = lifted.iter().find(|(name, _)| name == &key) else {
        return false;
    };
    let span = expr.span;
    // THE READER'S OWN BODY WINS. What the analyzer stamped on the
    // `config` chain is what it could say about a chain of unmodeled
    // hops — `Untyped` — while the reader's body is the value this now
    // calls. Keeping the chain's answer would leave the rewritten node
    // saying `untyped` about a method that plainly returns a Hash.
    let ty = reader_ty.clone().or_else(|| expr.ty.clone());
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(application),
            method: key,
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    expr.ty = ty;
    true
}

/// Peel a config READ back to its anchor, returning the
/// `Rails.application` expression and the key segments below `config`.
///
/// `Rails.application.config.app_version` → `(Rails.application,
/// ["app_version"])`; `Rails.configuration.x.vapid.public_key` →
/// `(Rails.application, ["x", "vapid", "public_key"])`. The caller joins
/// the segments with `_`, which is exactly how `config_receiver_path`
/// names the reader it lifted — the two halves meet at one name.
///
/// `Rails.configuration` is Rails' own alias for
/// `Rails.application.config`, so it anchors the same chain and the
/// `application` hop is synthesized to match.
fn peel_config_chain(expr: &Expr) -> Option<(Expr, Vec<String>)> {
    let mut segments: Vec<String> = Vec::new();
    let mut cur = expr;
    // Bounded so a pathological chain cannot spin; Rails' `x` namespace
    // is arbitrarily deep in principle but two levels in practice.
    for _ in 0..8 {
        let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*cur.node else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        match method.as_str() {
            "config" => {
                segments.reverse();
                return Some((r.clone(), segments));
            }
            "configuration" => {
                segments.reverse();
                let mut app = Expr::new(
                    r.span,
                    ExprNode::Send {
                        recv: Some(r.clone()),
                        method: crate::ident::Symbol::from("application"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                // The hop is synthesized after the analyzer has run, so
                // nothing else will ever type it. `Rails.application` is
                // `Untyped` in the stdlib registry — the answer the
                // analyzer would have given had the source written the
                // hop out — and an unstamped node here reads on the
                // ledger as `no known method application on Class(Rails)`
                // against a class that plainly has one.
                app.ty = Some(crate::ty::Ty::Untyped);
                return Some((app, segments));
            }
            _ => {
                segments.push(method.as_str().to_string());
                cur = r;
            }
        }
    }
    None
}
