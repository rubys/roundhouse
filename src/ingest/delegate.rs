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
#[derive(Debug)]
struct Delegation {
    method: Symbol,
    target: Symbol,
    name: String,
    allow_nil: bool,
    /// The real app file the `delegate` call was declared in, when the
    /// source registry can still resolve it (it can, at this point in
    /// the pipeline — see `ingest::sources`'s module doc). Empty when
    /// it can't; callers fall back to the pass's own `"<delegate>"`
    /// label for a synthesis-failure report rather than misattribute.
    file: String,
}

/// Expand every `delegate … to: …` the app's library classes declare.
///
/// Runs AFTER `lower_current_attributes`, which consumes the
/// declarations on its own classes — so what reaches here is the
/// general shape only.
pub fn lower_delegates(app: &mut crate::App) {
    let mut generated: Vec<(usize, Vec<MethodDef>)> = Vec::new();
    for (i, lc) in app.library_classes.iter_mut().enumerate() {
        let methods = expand_delegates_in_class(lc);
        if !methods.is_empty() {
            generated.push((i, methods));
        }
    }
    for (i, methods) in generated {
        app.library_classes[i].methods.extend(methods);
    }
}

/// `synthesized_source` produced Ruby Prism couldn't parse cleanly —
/// roundhouse's gap, not the app's. Record one survey entry naming
/// every delegated name in this class's declaration, attributed to the
/// real file when a delegated call's own span still resolves one.
fn record_synthesis_failure(delegates: &[Delegation], diags: &[crate::diagnostic::Diagnostic]) {
    let names = delegates
        .iter()
        .map(|d| d.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let file = delegates
        .iter()
        .map(|d| d.file.as_str())
        .find(|f| !f.is_empty())
        .unwrap_or("<delegate>")
        .to_string();
    super::survey::record_synthesis_failure(file, &format!("delegate forwarder for `{names}`"), diags);
}

/// The per-class body of `lower_delegates`, factored out so a caller
/// with a single `LibraryClass` in hand — rather than a whole `App` to
/// loop over — can drive the same expansion. `lower_controller_to_
/// library_class` is exactly that caller: a controller isn't a
/// `LibraryClass` at the point `lower_delegates` runs over
/// `app.library_classes` (ingest time; controllers only become one
/// per-target at emit time — see `lower::controller_to_library`), so a
/// `delegate` call in a controller body never reached this file at
/// all until the controller lowering started collecting it into its
/// own `unknown_calls` and calling this directly.
///
/// Returns the synthesized forwarder methods; does NOT append them to
/// `lc.methods` itself (`lower_delegates` above batches that across the
/// whole app; a single-class caller can just extend its own `methods`
/// with the result).
pub(crate) fn expand_delegates_in_class(lc: &mut LibraryClass) -> Vec<MethodDef> {
    let delegates = take_delegate_decls(lc);
    if delegates.is_empty() {
        return Vec::new();
    }
    let src = synthesized_source(lc, &delegates);

    // Isolated in its OWN scope — never the outer one that spans
    // the whole app's ingest — so a bug in `synthesized_source`
    // can't render its parse errors against an unrelated real
    // file (see `ingest::sources`'s module doc for how that
    // happened before this existed).
    let (parsed, diags) = crate::ingest::prism::scope(|| {
        crate::ingest::ingest_library_classes(src.as_bytes(), "<delegate>")
    });
    match parsed {
        Ok(classes) if diags.is_empty() => {
            classes.into_iter().flat_map(|c| c.methods).collect()
        }
        Ok(_) => {
            record_synthesis_failure(&delegates, &diags);
            Vec::new()
        }
        Err(err) => {
            super::survey::record(&err);
            Vec::new()
        }
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
        // them — see the module header. A setter is exempt: Ruby's own
        // assignment syntax fixes its arity at exactly one, so
        // `self.behavior = v` calling `behavior=` WITH an argument is
        // not evidence this pass can't cover it — it's what every
        // setter call looks like, delegated or not.
        if names.iter().any(|n| !n.as_str().ends_with('=') && called_with_args.contains(n.as_str())) {
            return true;
        }
        let file = super::sources::path_of(call.span.file).unwrap_or_default();
        for m in names {
            let name = if prefix {
                format!("{}_{}", target.as_str(), m.as_str())
            } else {
                m.as_str().to_string()
            };
            out.push(Delegation { method: m, target: target.clone(), name, allow_nil, file: file.clone() });
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
/// The receiver Rails writes for `to:`: `self.<to>` when the name is a
/// Ruby keyword (`to: :class`, a model's `return` association) or one of
/// the names its generated method uses itself, the name otherwise
/// (`DELEGATION_RESERVED_METHOD_NAMES` in active_support/delegation.rb).
fn receiver(target: &str) -> std::borrow::Cow<'_, str> {
    const RESERVED: &[&str] = &[
        "__ENCODING__", "__LINE__", "__FILE__", "alias", "and", "BEGIN", "begin", "break",
        "case", "class", "def", "defined?", "do", "else", "elsif", "END", "end", "ensure",
        "false", "for", "if", "in", "module", "next", "nil", "not", "or", "redo", "rescue",
        "retry", "return", "self", "super", "then", "true", "undef", "unless", "until", "when",
        "while", "yield", "_", "arg", "args", "block", "value", "key", "other", "__delegate_target",
    ];
    if RESERVED.contains(&target) {
        format!("self.{target}").into()
    } else {
        target.into()
    }
}

/// The forwarder for an operator method (`delegate :[], :<<, :==, to:
/// :@hash` — a collection wrapper's usual shape), or `None` for an
/// ordinary name. Unary operators (`!`, `-@`) forward as zero-arg sends
/// like any reader and are not listed.
fn operator_forwarder(t: &str, m: &str) -> Option<(&'static str, String)> {
    const BINARY: &[&str] = &[
        "==", "!=", "<", ">", "<=", ">=", "<=>", "===", "=~", "!~", "+", "-", "*", "/", "%", "**",
        "<<", ">>", "&", "|", "^",
    ];
    match m {
        "[]" => Some(("key", format!("{t}[key]"))),
        "[]=" => Some(("key, value", format!("{t}[key] = value"))),
        op if BINARY.contains(&op) => Some(("other", format!("{t} {op} other"))),
        _ => None,
    }
}

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
        let (target, m) = (receiver(d.target.as_str()), d.method.as_str());
        let t = if d.allow_nil { "__delegate_target" } else { &target };
        // A setter takes the one argument Ruby's own assignment syntax
        // supplies (with `prefix: true`, `d.name` is already
        // `<prefix>_name=`); an operator takes its fixed operands.
        let (params, call) = if let Some(forwarder) = operator_forwarder(t, m) {
            forwarder
        } else if let Some(attr) = m.strip_suffix('=') {
            ("value", format!("{t}.{attr} = value"))
        } else {
            ("", format!("{t}.{m}"))
        };
        let signature = if params.is_empty() {
            d.name.clone()
        } else {
            format!("{}({params})", d.name)
        };
        // Rails evaluates the receiver once. Nil's own operators still run
        // under allow_nil (for example `nil == other` returns a Boolean).
        // An `if`, not `return nil if …`: it leaves the method ending in a
        // read, which is what the strict targets want of a non-void body.
        let nil_operator = matches!(m, "==" | "!=" | "===" | "=~" | "!~" | "&" | "|" | "^" | "!");
        let call = if d.allow_nil {
            let result = if nil_operator {
                call
            } else {
                format!("if {t}.nil?\n      nil\n    else\n      {call}\n    end")
            };
            format!("{t} = {target}\n    {result}")
        } else {
            call
        };
        body.push_str(&format!("  def {signature}\n    {call}\n  end\n\n"));
    }
    // The class name is irrelevant — only the METHODS are lifted out of
    // the parse — but a wrapper is needed for the bodies to be methods.
    format!("class {}\n{body}end\n", lc.name.0.as_str().replace("::", "__"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::AccessorKind;
    use crate::effect::EffectSet;
    use crate::span::Span;

    fn library_class(src: &str) -> LibraryClass {
        crate::ingest::ingest_library_class(src.as_bytes(), "app/lib/deprecation.rb")
            .unwrap()
            .unwrap()
    }

    /// `synthesized_source`'s output, re-parsed the same way
    /// `lower_delegates` re-parses it — the same round trip that
    /// catches Bug A (a setter `def` with no parameter and a bare
    /// `x.y=` call is two syntax errors, not zero).
    fn synthesized_methods(lc: &LibraryClass, delegates: &[Delegation]) -> Vec<MethodDef> {
        let src = synthesized_source(lc, delegates);
        crate::ingest::ingest_library_classes(src.as_bytes(), "<test>")
            .unwrap_or_else(|e| panic!("synthesized source failed to parse: {e}\n{src}"))
            .into_iter()
            .flat_map(|c| c.methods)
            .collect()
    }

    #[test]
    fn a_delegated_getter_and_setter_pair_both_synthesize() {
        let mut lc = library_class(
            "class Deprecation\n  attr_accessor :deprecator\n\n  delegate :behavior, :behavior=, to: :deprecator\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(delegates.len(), 2);
        assert!(lc.unknown_calls.is_empty(), "the declaration should be consumed");

        let methods = synthesized_methods(&lc, &delegates);
        let getter = methods
            .iter()
            .find(|m| m.name.as_str() == "behavior")
            .expect("getter should be synthesized");
        assert!(getter.params.is_empty(), "a getter forwarder takes no arguments");

        let setter = methods
            .iter()
            .find(|m| m.name.as_str() == "behavior=")
            .expect("setter should be synthesized");
        assert_eq!(setter.params.len(), 1, "a setter forwarder takes exactly the one argument Ruby's own assignment syntax supplies");
    }

    #[test]
    fn prefix_true_composes_with_the_setter_name_too() {
        let mut lc = library_class(
            "class Deprecation\n  attr_accessor :deprecator\n\n  delegate :behavior=, to: :deprecator, prefix: true\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(delegates.len(), 1);
        assert_eq!(delegates[0].name, "deprecator_behavior=", "prefix composes ahead of the delegated name, trailing `=` intact");

        let methods = synthesized_methods(&lc, &delegates);
        let setter = methods
            .iter()
            .find(|m| m.name.as_str() == "deprecator_behavior=")
            .expect("prefixed setter should be synthesized under its prefixed name");
        assert_eq!(setter.params.len(), 1);
    }

    #[test]
    fn allow_nil_setter_ends_in_a_nil_guard() {
        let mut lc = library_class(
            "class Deprecation\n  attr_accessor :deprecator\n\n  delegate :behavior=, to: :deprecator, allow_nil: true\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(delegates.len(), 1);
        assert!(delegates[0].allow_nil);

        let methods = synthesized_methods(&lc, &delegates);
        let setter = methods
            .iter()
            .find(|m| m.name.as_str() == "behavior=")
            .expect("allow_nil setter should still be synthesized");
        assert_eq!(setter.params.len(), 1);
        // The receiver is read once into a local, then a nil guard (an
        // `If`) rather than a bare Assign ends the body — the same "ends
        // in a read" shape the getter's own allow_nil branch keeps.
        let ExprNode::Seq { exprs } = &*setter.body.node else {
            panic!("expected receiver-then-guard, got {:?}", setter.body.node)
        };
        assert!(
            matches!(exprs.last().map(|e| &*e.node), Some(ExprNode::If { .. })),
            "expected the allow_nil guard to end the body, got {:?}",
            setter.body.node
        );
    }

    #[test]
    fn bracket_assign_forwards_both_operands() {
        // `[]=` ends in `=` without being a one-value writer: it takes
        // (key, value), and forwards both.
        let mut lc = library_class(
            "class Store\n  attr_accessor :backing\n\n  delegate :[]=, to: :backing\nend\n",
        );
        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(delegates.len(), 1);
        let methods = synthesized_methods(&lc, &delegates);
        let setter = methods
            .iter()
            .find(|m| m.name.as_str() == "[]=")
            .expect("[]= should be synthesized");
        assert_eq!(setter.params.len(), 2, "[]= forwards its key and its value");
    }

    /// Ordinary Ruby can't actually produce a `Send { recv: None,
    /// method: "behavior=", .. }` node — `self.behavior = v` always
    /// carries an explicit receiver; a bare `behavior = v` is *always*
    /// local-variable assignment in Ruby, never a method send,
    /// delegated or not. So `names_called_with_arguments` (which only
    /// collects receiverless sends) can never actually hold a
    /// `"name="` key from real source today, and this guard's setter
    /// exemption is not reachable through the parser as things stand.
    /// It is still the right rule to state — the arity a `def name=`
    /// forwarder needs is fixed by Ruby's own assignment grammar, not
    /// by anything a call site can be observed doing — and cheap
    /// insurance against a future IR change (or a metaprogrammed call
    /// site this ingest doesn't model yet) making such a node
    /// possible. Pinned directly against hand-built IR since the
    /// parser has no way to hand us this shape.
    #[test]
    fn a_setter_name_is_exempt_from_the_called_with_arguments_guard() {
        let mut lc = library_class(
            "class Deprecation\n  attr_accessor :deprecator\n\n  delegate :behavior=, to: :deprecator\nend\n",
        );
        lc.methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            name: Symbol::from("reset!"),
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            block_param: None,
            name_span: Span::synthetic(),
            body: Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("behavior="),
                    args: vec![Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Sym { value: Symbol::from("warn") } },
                    )],
                    block: None,
                    parenthesized: false,
                },
            ),
            signature: None,
            effects: EffectSet::default(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        });

        let delegates = take_delegate_decls(&mut lc);
        assert_eq!(
            delegates.len(),
            1,
            "a setter's fixed arity exempts it from the call-site-arguments heuristic"
        );
    }
}
