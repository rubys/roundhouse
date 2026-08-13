//! `class Current < ActiveSupport::CurrentAttributes` → a plain,
//! statically-resolvable class.
//!
//! Rails' CurrentAttributes is three pieces of metaprogramming stacked:
//! `attribute :a, :b` defines instance accessors into a generated module,
//! the class-level `Current.user` is `method_missing` forwarding to a
//! per-thread singleton, and an app's own `def user=` reaches the
//! generated writer through `super`. None of that survives into an
//! emitted tree — the runtime has no `ActiveSupport::CurrentAttributes`,
//! and a target that resolves calls statically cannot follow
//! `method_missing` at all ([[feedback_runtime_must_be_statically_resolvable]]).
//!
//! campfire routes essentially everything through it (`Current.user`,
//! `Current.account`, `Current.session`), so this is load-bearing rather
//! than a nicety: without it the class replays verbatim and every
//! `Current.x` is unresolvable.
//!
//! ## What it becomes
//!
//! * one `@__instance` singleton plus `self.instance` / `self.reset`;
//! * a reader and writer per declared attribute, storing in an ivar;
//! * `delegate … to: :x, prefix: true, allow_nil: true` expanded to real
//!   methods (`request_host` → `@request.nil? ? nil : @request.host`);
//! * a `self.<name>` forwarder for every instance method, which is what
//!   makes the class-level call sites app code actually writes resolve.
//!
//! ## `super` in an app-written writer
//!
//! campfire writes `def session=(value); super(value); …; end`. Rather
//! than synthesize a module for the app's `super` to find — a mixin is
//! Ruby-family-only, and the strict targets have no equivalent — the
//! `super` call is REWRITTEN to the storage write it means (`@session =
//! value`). Same reasoning as the concern splices: resolve it in the IR
//! once, for all thirteen targets.

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

const BASE: &str = "ActiveSupport::CurrentAttributes";

/// Rewrite every `Current`-shaped class in the app.
pub fn lower_current_attributes(app: &mut crate::App) {
    let mut generated: Vec<(usize, Vec<MethodDef>)> = Vec::new();
    let mut lowered: Vec<ClassId> = Vec::new();
    for (i, lc) in app.library_classes.iter_mut().enumerate() {
        if lc.parent.as_ref().map(|p| p.0.as_str()) != Some(BASE) {
            continue;
        }
        let attrs = take_attribute_decls(lc);
        let delegates = take_delegate_decls(lc);
        rewrite_super_writers(lc, &attrs);
        let src = synthesized_source(lc, &attrs, &delegates);
        let methods = match crate::ingest::ingest_library_classes(src.as_bytes(), "<current>") {
            Ok(classes) => classes.into_iter().flat_map(|c| c.methods).collect(),
            Err(err) => {
                super::survey::record(&err);
                Vec::new()
            }
        };
        // The base is gone: nothing is inherited any more. Record the
        // name first — clearing the parent is exactly what makes this
        // class unrecognizable to everything downstream.
        lc.parent = None;
        lowered.push(lc.name.clone());
        generated.push((i, methods));
    }
    for (i, methods) in generated {
        app.library_classes[i].methods.extend(methods);
    }
    app.current_attribute_classes = lowered;
}

/// `attribute :session, :user, :request` — consumed, not replayed.
fn take_attribute_decls(lc: &mut LibraryClass) -> Vec<Symbol> {
    let mut out = Vec::new();
    lc.unknown_calls.retain(|call| {
        let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { return true };
        if method.as_str() != "attribute" {
            return true;
        }
        for a in args {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node {
                out.push(value.clone());
            }
        }
        false
    });
    out
}

/// `delegate :host, :protocol, to: :request, prefix: true, allow_nil: true`.
/// Returns `(method, target, generated_name, allow_nil)`.
struct Delegation {
    method: Symbol,
    target: Symbol,
    name: String,
    allow_nil: bool,
}

fn take_delegate_decls(lc: &mut LibraryClass) -> Vec<Delegation> {
    let mut out = Vec::new();
    lc.unknown_calls.retain(|call| {
        let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { return true };
        if method.as_str() != "delegate" {
            return true;
        }
        let mut names: Vec<Symbol> = Vec::new();
        let (mut to, mut prefix, mut allow_nil) = (None, false, false);
        for a in args {
            match &*a.node {
                ExprNode::Lit { value: Literal::Sym { value } } => names.push(value.clone()),
                ExprNode::Hash { entries, .. } => {
                    for (k, v) in entries {
                        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                            continue;
                        };
                        match key.as_str() {
                            "to" => {
                                if let ExprNode::Lit { value: Literal::Sym { value } } = &*v.node {
                                    to = Some(value.clone());
                                }
                            }
                            "prefix" => prefix = matches!(&*v.node,
                                ExprNode::Lit { value: Literal::Bool { value: true } }),
                            "allow_nil" => allow_nil = matches!(&*v.node,
                                ExprNode::Lit { value: Literal::Bool { value: true } }),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        // Only the shape we can reproduce exactly. Anything else stays
        // in `unknown_calls`, visible, rather than half-expanded.
        let Some(target) = to else { return true };
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

/// `super(value)` inside an app-written `<attr>=` is the generated
/// storage write. Rewrite it in place so no superclass is needed.
fn rewrite_super_writers(lc: &mut LibraryClass, attrs: &[Symbol]) {
    for m in &mut lc.methods {
        if m.receiver != MethodReceiver::Instance {
            continue;
        }
        let Some(attr) = m.name.as_str().strip_suffix('=') else { continue };
        if !attrs.iter().any(|a| a.as_str() == attr) {
            continue;
        }
        let ivar = Symbol::from(attr);
        rewrite_super(&mut m.body, &ivar);
    }
}

fn rewrite_super(expr: &mut Expr, ivar: &Symbol) {
    expr.node.for_each_child_mut(&mut |c| rewrite_super(c, ivar));
    let ExprNode::Super { args } = &*expr.node else { return };
    // `super` with no argument list (`ZSuper`) forwards the method's own
    // params; campfire writes the explicit `super(value)` form, and only
    // that is reproduced — a bare `super` keeps its shape and stays
    // visible rather than being guessed at.
    let Some(value) = args.as_ref().and_then(|a| a.first()).cloned() else { return };
    let span = expr.span;
    *expr = Expr::new(
        span,
        ExprNode::Assign {
            target: crate::expr::LValue::Ivar { name: ivar.clone() },
            value,
        },
    );
}

/// Build the synthesized half as Ruby and let ingest parse it, rather
/// than hand-assembling three dozen `Expr` trees.
fn synthesized_source(
    lc: &LibraryClass,
    attrs: &[Symbol],
    delegates: &[Delegation],
) -> String {
    let class = lc.name.0.as_str();
    let defines = |name: &str| {
        lc.methods
            .iter()
            .any(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == name)
    };

    let mut body = String::new();
    body.push_str(&format!(
        "  def self.instance\n    @__instance = {class}.new if @__instance.nil?\n    @__instance\n  end\n\n\
         \x20 def self.reset\n    @__instance = nil\n    nil\n  end\n\n"
    ));

    for a in attrs {
        let a = a.as_str();
        if !defines(a) {
            body.push_str(&format!("  def {a}\n    @{a}\n  end\n\n"));
        }
        if !defines(&format!("{a}=")) {
            body.push_str(&format!("  def {a}=(value)\n    @{a} = value\n  end\n\n"));
        }
    }

    for d in delegates {
        if defines(&d.name) {
            continue;
        }
        let (t, m) = (d.target.as_str(), d.method.as_str());
        if d.allow_nil {
            body.push_str(&format!(
                "  def {}\n    @{t}.nil? ? nil : @{t}.{m}\n  end\n\n",
                d.name
            ));
        } else {
            body.push_str(&format!("  def {}\n    @{t}.{m}\n  end\n\n", d.name));
        }
    }

    // Class-level forwarders — the surface app code actually calls
    // (`Current.user`, `Current.account`). Rails gets these from
    // `method_missing`; a statically-resolved target needs them real.
    let mut forwarded: Vec<String> = Vec::new();
    for a in attrs {
        forwarded.push(a.as_str().to_string());
        forwarded.push(format!("{}=", a.as_str()));
    }
    for d in delegates {
        forwarded.push(d.name.clone());
    }
    for m in &lc.methods {
        if m.receiver == MethodReceiver::Instance {
            forwarded.push(m.name.as_str().to_string());
        }
    }
    forwarded.sort();
    forwarded.dedup();
    for name in forwarded {
        if let Some(attr) = name.strip_suffix('=') {
            body.push_str(&format!(
                "  def self.{attr}=(value)\n    {class}.instance.{attr} = value\n  end\n\n"
            ));
        } else {
            body.push_str(&format!(
                "  def self.{name}\n    {class}.instance.{name}\n  end\n\n"
            ));
        }
    }

    format!("class {class}\n{body}end\n")
}

