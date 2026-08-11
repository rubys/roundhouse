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
    let lifted: Vec<crate::ident::Symbol> = match &app.rails_application {
        Some(lc) => lc.methods.iter().map(|m| m.name.clone()).collect(),
        None => return,
    };
    if lifted.is_empty() {
        return;
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &lifted));
    let lifted_for_views = lifted.clone();
    for view in &mut app.views {
        rewrite(&mut view.body, &lifted_for_views);
    }
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

fn rewrite(expr: &mut Expr, lifted: &[crate::ident::Symbol]) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, lifted));

    let ExprNode::Send { recv: Some(config_call), method: key, args, .. } = &*expr.node else {
        return;
    };
    if !args.is_empty() || !lifted.contains(key) {
        return;
    }
    // The receiver has to be the `config` hop itself.
    let ExprNode::Send { recv: Some(app_call), method: config, args: cargs, .. } =
        &*config_call.node
    else {
        return;
    };
    if config.as_str() != "config" || !cargs.is_empty() {
        return;
    }
    let application = app_call.clone();
    let key = key.clone();
    let span = expr.span;
    let ty = expr.ty.clone();
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
}
