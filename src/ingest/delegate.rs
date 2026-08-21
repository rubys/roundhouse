//! ActiveSupport's `delegate :a, :b, to: :target` → real methods.
//!
//! Rails defines these with `class_eval` at load time, so nothing about
//! them survives into an emitted tree: the declaration lands in
//! `unknown_calls` and every call to a delegated name is a bare send no
//! class defines. The failure is SILENT wherever the caller rescues —
//! campfire's `message_presentation` wraps its whole body in `rescue
//! Exception` and returns `""`, so a missing `fragment` rendered every
//! message with an EMPTY body and no error anywhere. The search page
//! looked like it had no results; it had one, drawn blank.
//!
//! [`super::current_attributes`] already expands the declaration for
//! `ActiveSupport::CurrentAttributes` subclasses, where the target is a
//! declared attribute and the forwarder can read its ivar directly.
//! This is the general case: the target is a METHOD (campfire's
//! `attr_reader :content` beside the declaration), so the forwarder
//! CALLS it, which is also what Rails' generated body does.
//!
//! ## Where it declines
//!
//! A delegated method that takes ARGUMENTS. Rails forwards them with
//! `*args, &block`; argument forwarding is the shape the strict targets
//! do not lower ([[project_kwarg_forwarding_strict_targets_gap]]), and
//! a zero-arg forwarder for a method that takes two is an arity error
//! standing in for a NameError — a different wrong answer, not a fix.
//! The test is the declaring class's own call sites: campfire's
//! `Messages::AttachmentPresentation` delegates `:tag`, `:link_to` and
//! four more `to: :context` and calls every one of them WITH arguments
//! in the same file, so that declaration stays in `unknown_calls`,
//! visible, exactly as it is today.
//!
//! The limitation that leaves: a declaration in a BASE class whose only
//! argument-passing callers are subclasses reads as zero-arg here.
//! campfire has no such case — `ActionText::Content::Filter`'s
//! `fragment` is a reader on both sides — and closing it properly means
//! the arity coming from the TARGET's own signature rather than from
//! call sites.

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// One expanded `delegate` entry: `<name>` forwards to `<target>.<method>`.
struct Delegation {
    method: Symbol,
    target: Symbol,
    name: String,
    allow_nil: bool,
}

/// Expand every `delegate … to: …` the app's library classes declare.
///
/// Runs AFTER `lower_current_attributes`, which consumes the
/// declarations on its own classes — so what reaches here is the
/// general shape only.
pub fn lower_delegates(app: &mut crate::App) {
    let mut generated: Vec<(usize, Vec<MethodDef>)> = Vec::new();
    for (i, lc) in app.library_classes.iter_mut().enumerate() {
        let delegates = take_delegate_decls(lc);
        if delegates.is_empty() {
            continue;
        }
        let src = synthesized_source(lc, &delegates);
        let methods = match crate::ingest::ingest_library_classes(src.as_bytes(), "<delegate>") {
            Ok(classes) => classes.into_iter().flat_map(|c| c.methods).collect(),
            Err(err) => {
                super::survey::record(&err);
                Vec::new()
            }
        };
        generated.push((i, methods));
    }
    for (i, methods) in generated {
        app.library_classes[i].methods.extend(methods);
    }
}

/// Consume the declarations this pass can reproduce EXACTLY, leaving
/// every other shape in `unknown_calls` rather than half-expanded.
fn take_delegate_decls(lc: &mut LibraryClass) -> Vec<Delegation> {
    let called_with_args = names_called_with_arguments(lc);
    let mut out = Vec::new();
    lc.unknown_calls.retain(|call| {
        let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { return true };
        if method.as_str() != "delegate" {
            return true;
        }
        let mut names: Vec<Symbol> = Vec::new();
        let (mut to, mut prefix, mut allow_nil) = (None, false, false);
        let mut unknown_option = false;
        for a in args {
            match &*a.node {
                ExprNode::Lit { value: Literal::Sym { value } } => names.push(value.clone()),
                ExprNode::Hash { entries, .. } => {
                    for (k, v) in entries {
                        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                            unknown_option = true;
                            continue;
                        };
                        match key.as_str() {
                            "to" => {
                                if let ExprNode::Lit { value: Literal::Sym { value } } = &*v.node {
                                    to = Some(value.clone());
                                } else {
                                    unknown_option = true;
                                }
                            }
                            "prefix" => {
                                prefix = matches!(&*v.node,
                                    ExprNode::Lit { value: Literal::Bool { value: true } });
                                if !prefix {
                                    unknown_option = true;
                                }
                            }
                            "allow_nil" => allow_nil = matches!(&*v.node,
                                ExprNode::Lit { value: Literal::Bool { value: true } }),
                            // `private:`, `prefix: :other_name` and the
                            // rest are shapes this does not reproduce.
                            _ => unknown_option = true,
                        }
                    }
                }
                _ => unknown_option = true,
            }
        }
        let Some(target) = to else { return true };
        if unknown_option || names.is_empty() {
            return true;
        }
        // Arguments at a call site mean the forwarder needs to forward
        // them — see the module header.
        if names.iter().any(|n| called_with_args.contains(n.as_str())) {
            return true;
        }
        for m in names {
            let name = if prefix {
                format!("{}_{}", target.as_str(), m.as_str())
            } else {
                m.as_str().to_string()
            };
            out.push(Delegation { method: m, target: target.clone(), name, allow_nil });
        }
        false
    });
    out
}

/// Bare names this class calls WITH arguments, anywhere in its own
/// method bodies. A delegated name in this set needs the argument
/// forwarding this pass declines to synthesize.
fn names_called_with_arguments(lc: &LibraryClass) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for m in &lc.methods {
        collect_calls_with_args(&m.body, &mut out);
    }
    out
}

fn collect_calls_with_args(expr: &Expr, out: &mut std::collections::HashSet<String>) {
    expr.node.for_each_child(&mut |c| collect_calls_with_args(c, out));
    let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node else { return };
    if !args.is_empty() || block.is_some() {
        out.insert(method.as_str().to_string());
    }
}

/// The forwarders, as Ruby source — parsed back through ingest so the
/// generated bodies are ordinary IR, indistinguishable from a method
/// the app wrote. Same construction `current_attributes` uses.
fn synthesized_source(lc: &LibraryClass, delegates: &[Delegation]) -> String {
    let defines = |name: &str| {
        lc.methods
            .iter()
            .any(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == name)
    };
    let mut body = String::new();
    for d in delegates {
        if defines(&d.name) {
            continue;
        }
        let (t, m) = (d.target.as_str(), d.method.as_str());
        if d.allow_nil {
            // A ternary, not `return nil if …`: it leaves the method
            // ending in a read, which is what the strict targets want
            // of a non-void body.
            body.push_str(&format!("  def {}\n    {t}.nil? ? nil : {t}.{m}\n  end\n\n", d.name));
        } else {
            body.push_str(&format!("  def {}\n    {t}.{m}\n  end\n\n", d.name));
        }
    }
    // The class name is irrelevant — only the METHODS are lifted out of
    // the parse — but a wrapper is needed for the bodies to be methods.
    format!("class {}\n{body}end\n", lc.name.0.as_str().replace("::", "__"))
}
