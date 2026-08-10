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

use crate::app::App;
use crate::expr::{Expr, ExprNode};

pub fn apply_config_reader_lowering(app: &mut App) {
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
