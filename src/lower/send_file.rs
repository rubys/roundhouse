//! `send_file path, content_type:, disposition:` → `send_data
//! File.binread(path), type:, disposition:`.
//!
//! Rails streams the file at `path` as the response body. This grounds
//! it to the send the runtime already has, by reading the file AT THE
//! CALL SITE — which is the same move `attached::apply_attach_lowering`
//! makes on `attach(io:)`, and for the same reason.
//!
//! # Why not a runtime method
//!
//! `runtime/ruby/` does no file I/O anywhere, and that is a property
//! rather than an accident: every file under it transpiles to EVERY
//! target, so a `File.binread` in `action_controller/base.rb` is a
//! primitive nine strict runtimes have to grow before an app that never
//! calls `send_file` can compile. The runtime's RBS has no `File` type
//! to name either, so the same body is five new `Ty::Untyped` sites in
//! the shared corpus — measured, not predicted: the version that lived
//! in base.rb tripped `runtime_src_integration`'s ceiling by one.
//!
//! Lowered here instead, `File.binread` lands in the APP's own emitted
//! code, where the analyzer's stdlib registry already types it and only
//! the apps that actually send files pay for it.
//!
//! # What is admitted
//!
//! The path, plus `content_type:`/`type:` and `disposition:` as literal
//! options. Rails accepts both spellings of the type (`send_file_headers!`
//! reads `:type`; `send_file` itself honours an explicit `:content_type`),
//! and campfire's account logo writes the latter. A `disposition:` that
//! is not a Symbol or String literal, or any option this does not
//! reproduce (`:filename`, `:status`, `:url_based_filename`), leaves the
//! call ALONE — it then fails by name, which is the honest ledger entry
//! rather than a response quietly missing a header.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_send_file_lowering(app: &mut crate::app::App) {
    super::for_each_hook_body(app, &mut rewrite_send_file);
}

/// Options Rails' `send_file` takes that this lowering does not carry.
/// Seeing one is a refusal, not a drop.
const UNMODELED_OPTIONS: &[&str] = &["filename", "status", "url_based_filename", "stream", "buffer_size"];

fn rewrite_send_file(e: &mut Expr) {
    e.node.for_each_child_mut(&mut rewrite_send_file);
    let span = e.span;
    let ExprNode::Send { recv, method, args, block, .. } = &*e.node else { return };
    if method.as_str() != "send_file" || args.is_empty() || args.len() > 2 || block.is_some() {
        return;
    }
    // Implicit self or `self.` — a `send_file` on some other object is
    // not the controller's.
    match recv {
        None => {}
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => {}
        _ => return,
    }

    let mut content_type: Option<Expr> = None;
    let mut disposition: Option<Expr> = None;
    if let Some(opts) = args.get(1) {
        let ExprNode::Hash { entries, kwargs: true } = &*opts.node else { return };
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else { return };
            match name.as_str() {
                // `content_type:` wins over `type:` when both are given,
                // as Rails' own `self.content_type = …` assignment does
                // (it runs after `send_file_headers!`).
                "content_type" => content_type = Some(v.clone()),
                "type" if content_type.is_none() => content_type = Some(v.clone()),
                "disposition" => match &*v.node {
                    ExprNode::Lit { value: Literal::Sym { value: d } } => {
                        disposition = Some(lit_str(span, d.as_str()));
                    }
                    ExprNode::Lit { value: Literal::Str { .. } } => disposition = Some(v.clone()),
                    _ => return,
                },
                other if UNMODELED_OPTIONS.contains(&other) => return,
                _ => return,
            }
        }
    }

    let path = args[0].clone();
    let data = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("File")] },
            )),
            method: Symbol::from("binread"),
            args: vec![path],
            block: None,
            parenthesized: true,
        },
    );
    let mut entries: Vec<(Expr, Expr)> = vec![(
        lit_sym(span, "type"),
        content_type.unwrap_or_else(|| lit_str(span, "application/octet-stream")),
    )];
    entries.push((
        lit_sym(span, "disposition"),
        disposition.unwrap_or_else(|| lit_str(span, "attachment")),
    ));
    *e = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("send_data"),
            args: vec![data, Expr::new(span, ExprNode::Hash { entries, kwargs: true })],
            block: None,
            parenthesized: true,
        },
    );
}

fn lit_str(span: crate::span::Span, s: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } })
}

fn lit_sym(span: crate::span::Span, s: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
}
