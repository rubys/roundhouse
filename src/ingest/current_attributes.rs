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
//! * one thread-local singleton plus `self.instance` / `self.reset`;
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
//!
//! Only the no-default/no-reset-callback subset is flattened. Explicit
//! defaults (including nil) and callback declarations retain an error
//! annotation and their original base; dropping them is not reset support.

use crate::diagnostic::DiagnosticKind;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

const BASE: &str = "ActiveSupport::CurrentAttributes";

/// Rewrite every `Current`-shaped class in the app.
pub fn lower_current_attributes(app: &mut crate::App) {
    let mut generated: Vec<(usize, Vec<MethodDef>)> = Vec::new();
    let mut lowered: Vec<ClassId> = Vec::new();
    let mut signatures: Vec<(ClassId, Symbol, crate::ty::Ty)> = Vec::new();
    let owners: std::collections::BTreeSet<ClassId> = app.library_classes.iter()
        .filter(|lc| lc.parent.as_ref().map(|p| p.0.as_str()) == Some(BASE))
        .map(|lc| lc.name.clone()).collect();
    let mut unsupported_owners = std::collections::BTreeSet::new();
    // A reopen can add lifecycle declarations or reset super after the
    // fragment that names the base. Refuse the owner before removing it.
    for lc in app.library_classes.iter_mut().filter(|lc| owners.contains(&lc.name)) {
        let mut unsupported = false;
        for call in &mut lc.unknown_calls {
            unsupported |= refuse_lifecycle(call);
        }
        for method in &mut lc.methods {
            if method.name.as_str() == "reset" {
                unsupported |= refuse_reset_super(&mut method.body);
            }
        }
        if unsupported {
            unsupported_owners.insert(lc.name.clone());
        }
    }
    for (i, lc) in app.library_classes.iter_mut().enumerate() {
        if lc.parent.as_ref().map(|p| p.0.as_str()) != Some(BASE)
            || unsupported_owners.contains(&lc.name)
        {
            continue;
        }
        let (attrs, declared) = take_attribute_decls(lc);
        let delegates = take_delegate_decls(lc);
        rewrite_super_accessors(lc, &attrs);
        let src = synthesized_source(lc, &attrs, &delegates);
        // Isolated in its own `prism::scope` — never the outer one
        // that spans the whole app's ingest — so a bug in
        // `synthesized_source` can't render its parse errors against
        // an unrelated real file (see `ingest::sources`'s module doc).
        let (parsed, diags) = crate::ingest::prism::scope(|| {
            crate::ingest::ingest_library_classes(src.as_bytes(), "<current>")
        });
        let methods = match parsed {
            Ok(classes) if diags.is_empty() => {
                let mut methods: Vec<MethodDef> = classes.into_iter().flat_map(|c| c.methods).collect();
                for (name, signature) in declare_attribute_types(&mut methods, &declared) {
                    signatures.push((lc.name.clone(), name, signature));
                }
                methods
            }
            Ok(_) => {
                super::survey::record_synthesis_failure(
                    "<current>",
                    &format!("CurrentAttributes forwarder for `{}`", lc.name.0.as_str()),
                    &diags,
                );
                Vec::new()
            }
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
    // Declared like an inline signature, so the analysis reads the
    // written type in the class's lexical scope (`Context::…`
    // inside a namespaced `Current`).
    for (class, name, signature) in signatures {
        app.rbs_signatures.entry(class).or_default().entry(name).or_insert(signature);
    }
    app.current_attribute_classes = lowered;
}

fn refuse_lifecycle(expr: &mut Expr) -> bool {
    let mut refused = false;
    expr.node.for_each_child_mut(&mut |child| refused |= refuse_lifecycle(child));
    let ExprNode::Send { recv, method, args, .. } = &*expr.node else { return refused };
    if recv.as_ref().is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef)) {
        return refused;
    }
    let detail = match method.as_str() {
        "attribute" if !args.iter().all(|a| matches!(&*a.node,
            ExprNode::Lit { value: Literal::Sym { .. } })) =>
            "only literal attributes without explicit defaults are supported; default duplication/callable defaults and reset resolution are not implemented",
        "before_reset" | "after_reset" | "resets" | "set_callback" | "skip_callback" =>
            "reset callbacks are not implemented; flattening would discard lifecycle effects",
        _ => return refused,
    };
    expr.diagnostic = Some(DiagnosticKind::Unsupported {
        target: None,
        construct: Symbol::from("CurrentAttributes lifecycle"),
        detail: detail.into(),
    });
    true
}

fn refuse_reset_super(expr: &mut Expr) -> bool {
    let mut refused = false;
    expr.node.for_each_child_mut(&mut |child| refused |= refuse_reset_super(child));
    if matches!(&*expr.node, ExprNode::Super { .. }) {
        expr.diagnostic = Some(DiagnosticKind::Unsupported {
            target: None,
            construct: Symbol::from("CurrentAttributes reset"),
            detail: "reset super requires the inherited Rails lifecycle; flattening would remove its destination".into(),
        });
        return true;
    }
    refused
}

/// `attribute :session, :user, :request` — consumed, not replayed.
/// An attribute may carry its type in a trailing RBS comment
/// (`attribute :tenant #: Account::Context::Impl`),
/// which ingest reads as a cast around the call: that type is returned
/// with each name the call lists. Without unwrapping it the declaration was not taken,
/// the class got no accessor, and `Current.tenant` had no method.
fn take_attribute_decls(lc: &mut LibraryClass) -> (Vec<Symbol>, Vec<(Symbol, crate::ty::Ty)>) {
    let mut out = Vec::new();
    let mut declared = Vec::new();
    lc.unknown_calls.retain(|call| {
        let (call, declared_ty) = match &*call.node {
            ExprNode::Cast { value, target_ty } => (value, Some(target_ty)),
            _ => (call, None),
        };
        let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { return true };
        if method.as_str() != "attribute" {
            return true;
        }
        let names: Vec<Symbol> = args
            .iter()
            .filter_map(|a| match &*a.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                _ => None,
            })
            .collect();
        if let Some(ty) = declared_ty {
            declared.extend(names.iter().map(|name| (name.clone(), ty.clone())));
        }
        out.extend(names);
        false
    });
    (out, declared)
}

/// The synthesized reader answers, and the writer takes, an
/// attribute's declared type, nilable: every attribute reads nil until
/// the request sets it (and after `reset`), and callers ask exactly that
/// (`create_tenant unless Current.tenant`). Read as written,
/// the presence test would fold to true (see current_attributes_typing).
/// Returns each accessor's signature.
fn declare_attribute_types(methods: &mut [MethodDef], declared: &[(Symbol, crate::ty::Ty)]) -> Vec<(Symbol, crate::ty::Ty)> {
    use crate::ty::{Param, ParamKind, Ty};
    let mut out = Vec::new();
    for (name, written) in declared {
        let ty = &match written {
            Ty::Nil => Ty::Nil,
            Ty::Union { variants } if variants.contains(&Ty::Nil) => written.clone(),
            _ => Ty::Union { variants: vec![written.clone(), Ty::Nil] },
        };
        let writer = format!("{}=", name.as_str());
        for method in methods.iter_mut().filter(|m| m.receiver == MethodReceiver::Instance) {
            let params = if method.name == *name {
                vec![]
            } else if method.name.as_str() == writer {
                vec![Param { name: Symbol::from("value"), ty: ty.clone(), kind: ParamKind::Required }]
            } else {
                continue;
            };
            let signature = Ty::Fn { params, block: None, ret: Box::new(ty.clone()), effects: method.effects.clone() };
            method.signature = Some(signature.clone());
            out.push((method.name.clone(), signature));
        }
    }
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

/// Accessor overrides reach Rails' generated module through `super`.
/// Resolve the zero-argument reader and explicit one-argument writer.
fn rewrite_super_accessors(lc: &mut LibraryClass, attrs: &[Symbol]) {
    for m in &mut lc.methods {
        if m.receiver != MethodReceiver::Instance {
            continue;
        }
        let writer = m.name.as_str().ends_with('=');
        let attr = m.name.as_str().strip_suffix('=').unwrap_or(m.name.as_str());
        if !attrs.iter().any(|a| a.as_str() == attr) {
            continue;
        }
        let ivar = Symbol::from(attr);
        rewrite_super(&mut m.body, &ivar, writer, m.params.is_empty());
    }
}

fn rewrite_super(expr: &mut Expr, ivar: &Symbol, writer: bool, parameterless: bool) {
    expr.node.for_each_child_mut(&mut |c| rewrite_super(c, ivar, writer, parameterless));
    let ExprNode::Super { args } = &*expr.node else { return };
    if !writer {
        if args.as_ref().is_some_and(|a| !a.is_empty()) || (args.is_none() && !parameterless) {
            return;
        }
        *expr = Expr::new(expr.span, ExprNode::Ivar { name: ivar.clone() });
        return;
    }
    // `super` with no argument list (`ZSuper`) forwards the method's own
    // params; campfire writes the explicit `super(value)` form, and only
    // that is reproduced — a bare `super` keeps its shape and stays
    // visible rather than being guessed at.
    let Some(value) = args.as_ref().filter(|a| a.len() == 1).and_then(|a| a.first()).cloned() else { return };
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
    // ONE INSTANCE PER THREAD, which is what `CurrentAttributes` means:
    // Rails keeps the instance in `IsolatedExecutionState`, so two
    // requests served at once each see their own `Current.user`. A
    // class-level `@__instance` was one instance for the whole process
    // -- correct under the single-fiber server the binary used to run,
    // and a cross-request identity leak the moment two green threads (or
    // two Puma threads on the CRuby lane) dispatch at once. The slot is
    // `Thread.current[...]`, keyed per class so two CurrentAttributes
    // classes in one app do not share it; spinel types thread-local
    // storage by key from its assignments, the way it types ivars, so
    // the reader below narrows to the class and every forwarder stays
    // typed.
    //
    // The reader narrows its slot behind `is_a?`, not `nil?`, because
    // Thread#[] answers untyped. Reset must not replace this object or
    // call app writers: Rails replaces its declared storage directly.
    // With no explicit defaults/callbacks that clears each slot and
    // returns the empty resolved-defaults Hash, preserving unrelated ivars.
    let key = format!("__current_attrs_{}", class.replace("::", "_"));
    body.push_str(&format!(
        "  def self.instance\n    c = Thread.current[:{key}]\n    return c if c.is_a?({class})\n    c = {class}.new\n    Thread.current[:{key}] = c\n    c\n  end\n\n"
    ));
    if !defines("reset") {
        body.push_str("  def reset\n");
        for attr in attrs {
            body.push_str(&format!("    @{attr} = nil\n"));
        }
        body.push_str("    {}\n  end\n\n");
    }

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
    let mut forwarded: Vec<String> = vec!["reset".into()];
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
        if lc.methods.iter().any(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == name) {
            continue;
        }
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
