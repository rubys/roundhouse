//! Source admission for literal CurrentAttributes scopes.
//!
//! Shared by inference, source diagnostics and lowering. This module
//! describes source contracts only; generated execution belongs to
//! `lower::current_set`. The immutable hook-body walker is the existing
//! shared App inventory, not a lowering stage.

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::MethodReceiver;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;
use std::collections::{BTreeMap, BTreeSet};

/// Known framework dispatch, excluding source overrides and unknown ancestry.
pub(crate) struct ScopedClasses {
    pub(crate) supported: BTreeMap<ClassId, BTreeSet<String>>,
    represented: BTreeSet<ClassId>,
    ambiguous_relative: BTreeSet<String>,
}

pub(crate) fn scoped_classes(app: &App) -> ScopedClasses {
    let supported: BTreeMap<_, _> = app
        .current_attribute_classes
        .iter()
        .filter_map(|id| {
            let mut pending = vec![id.clone()];
            let mut seen = BTreeSet::new();
            let mut readers = BTreeSet::new();
            let mut writers = BTreeSet::new();
            while let Some(owner) = pending.pop() {
                if !seen.insert(owner.clone()) {
                    continue;
                }
                if let Some(class) = app.library_classes.iter().find(|class| class.name == owner) {
                    if class
                        .methods
                        .iter()
                        .any(|method| method.name.as_str() == "set")
                    {
                        return None;
                    }
                    for method in class
                        .methods
                        .iter()
                        .filter(|m| m.receiver == MethodReceiver::Instance)
                    {
                        if method.params.is_empty() {
                            readers.insert(method.name.to_string());
                        } else if method.params.len() == 1 {
                            if let Some(name) = method.name.as_str().strip_suffix('=') {
                                writers.insert(name.to_string());
                            }
                        }
                    }
                    pending.extend(class.includes.iter().cloned());
                } else {
                    // Include-only modules may not have a LibraryClass. Missing
                    // ancestry cannot establish framework dispatch.
                    return None;
                }
            }
            Some((
                id.clone(),
                readers.intersection(&writers).cloned().collect(),
            ))
        })
        .collect();
    // Reject-only ambiguity inventory, never suffix-based admission.
    let mut by_relative: BTreeMap<String, BTreeSet<&ClassId>> = BTreeMap::new();
    for id in app
        .library_classes
        .iter()
        .map(|c| &c.name)
        .chain(app.models.iter().map(|m| &m.name))
        .chain(app.controllers.iter().map(|c| &c.name))
    {
        let segments: Vec<_> = id.0.as_str().split("::").collect();
        for start in 0..segments.len() {
            by_relative
                .entry(segments[start..].join("::"))
                .or_default()
                .insert(id);
        }
    }
    let ambiguous_relative = by_relative
        .into_iter()
        .filter_map(|(path, ids)| {
            (ids.len() > 1 && ids.iter().any(|id| supported.contains_key(*id))).then_some(path)
        })
        .collect();
    let represented = app
        .library_classes
        .iter()
        .map(|c| c.name.clone())
        .chain(app.models.iter().map(|m| m.name.clone()))
        .chain(app.controllers.iter().map(|c| c.name.clone()))
        .collect();
    ScopedClasses {
        supported,
        represented,
        ambiguous_relative,
    }
}

pub(crate) fn literal_set_site(
    recv: Option<&Expr>,
    method: &Symbol,
    args: &[Expr],
    block: Option<&Expr>,
    classes: &ScopedClasses,
) -> Option<(ClassId, Vec<(String, Expr)>)> {
    let site = classify_site(recv, method, args, block, classes)?;
    if site.refusal.is_some() {
        return None;
    }
    Some((site.class?, site.pairs))
}

pub(crate) struct LiteralSite {
    pub(crate) class: Option<ClassId>,
    pub(crate) pairs: Vec<(String, Expr)>,
    pub(crate) refusal: Option<(Span, &'static str)>,
}

/// One source admission policy for inference, lowering and output boundaries.
pub(crate) fn classify_site(
    recv: Option<&Expr>,
    method: &Symbol,
    args: &[Expr],
    block: Option<&Expr>,
    classes: &ScopedClasses,
) -> Option<LiteralSite> {
    if method.as_str() != "set" {
        return None;
    }
    let block = block?;
    let ExprNode::Lambda { .. } = &*block.node else {
        return None;
    };
    let pairs = keyword_pairs(args)?;
    // Ordinary constant resolution wins when the analyzer already named a
    // represented class. Relative spelling is only a refusal for untyped IR,
    // the same as any other unresolved constant, not a Current.set inventory.
    let relative = recv.and_then(|recv| match &*recv.node {
        ExprNode::Const { path } if path.first().is_some_and(|part| !part.as_str().is_empty()) => {
            Some((
                recv,
                path.iter()
                    .map(|part| part.as_str())
                    .collect::<Vec<_>>()
                    .join("::"),
            ))
        }
        _ => None,
    });
    if let Some((recv, written)) = &relative {
        let typed = match &recv.ty {
            Some(Ty::Class { id, .. }) => classes.represented.contains(id),
            _ => false,
        };
        if !typed && classes.ambiguous_relative.contains(written) {
            return Some(LiteralSite {
                class: None,
                pairs,
                refusal: Some((
                    recv.span,
                    "ambiguous relative CurrentAttributes receiver requires explicit qualification",
                )),
            });
        }
        if !typed
            && !classes
                .represented
                .contains(&ClassId(Symbol::from(written.as_str())))
            && classes
                .supported
                .keys()
                .any(|id| id.0.as_str().ends_with(&format!("::{written}")))
        {
            return Some(LiteralSite {
                class: None,
                pairs,
                refusal: Some((
                    recv.span,
                    "unresolved relative CurrentAttributes receiver requires explicit qualification",
                )),
            });
        }
    }
    let class = set_receiver_class(recv, method, classes)?;
    let mut keys = BTreeSet::new();
    let refusal = if pairs.iter().any(|(name, _)| !keys.insert(name)) {
        Some((
            args[0].span,
            "duplicate literal attribute keys are not supported",
        ))
    } else if pairs
        .iter()
        .any(|(name, _)| !classes.supported[&class].contains(name))
    {
        Some((
            args[0].span,
            "literal keys require a represented instance reader and writer",
        ))
    } else {
        block_refusal(block)
    };
    Some(LiteralSite {
        class: Some(class),
        pairs,
        refusal,
    })
}

fn block_refusal(block: &Expr) -> Option<(Span, &'static str)> {
    let ExprNode::Lambda {
        rest_param,
        block_param,
        has_unrepresented_bindings,
        from_block_pass,
        ..
    } = &*block.node
    else {
        return Some((block.span, "only an attached literal block is supported"));
    };
    if *from_block_pass {
        return Some((
            block.span,
            "converted or forwarded block operands are not supported",
        ));
    }
    if *has_unrepresented_bindings {
        return Some((
            block.span,
            "the source block has bindings not preserved by the compiler",
        ));
    }
    if rest_param.is_some() || block_param.is_some() {
        return Some((
            block.span,
            "rest and block parameters are outside the supported scope",
        ));
    }
    // Nested Current.set sites are classified independently. Nested
    // loops, iterators and closures stay ordinary Ruby on the original
    // attached block and do not refuse this site.
    None
}

pub(crate) fn for_each_body(app: &App, f: &mut impl FnMut(&Expr, Option<&'static str>)) {
    let initializing: Vec<_> = app
        .library_classes
        .iter()
        .filter(|class| app.current_attribute_classes.contains(&class.name))
        .flat_map(|class| {
            class
                .constants
                .iter()
                .map(|(_, value)| value)
                .chain(class.unknown_calls.iter())
        })
        .collect();
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        let reason = initializing.iter().any(|initializer| std::ptr::eq(*initializer, body))
            .then_some("CurrentAttributes class-body initializers are outside Current.set execution coverage");
        f(body, reason);
    });
    for view in &app.views {
        f(&view.body, None);
    }
    // Inventory only: these containers are synthesized later or have no
    // mutable Current traversal. Refuse, do not expand execution support.
    let unlowered = Some("this source container is outside Current.set execution coverage");
    for helper in &app.routes.direct_helpers {
        f(&helper.body, unlowered);
    }
    for controller in &app.controllers {
        for item in &controller.body {
            if let crate::dialect::ControllerBodyItem::Action { action, .. } = item {
                for (_, default) in &action.kw_params {
                    if let Some(default) = default {
                        f(default, unlowered);
                    }
                }
            }
        }
    }
    for view in &app.views {
        for param in view.strict_locals.iter().flatten() {
            if let Some(default) = &param.default {
                f(default, unlowered);
            }
        }
    }
    for fixture in &app.fixtures {
        for expr in &fixture.preamble {
            f(expr, unlowered);
        }
        for record in fixture.records.values() {
            for value in record.values() {
                if let crate::dialect::FixtureValue::Ruby(expr) = value {
                    f(expr, unlowered);
                }
            }
        }
    }
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Association { assoc, .. } = item {
                let body = match assoc {
                    crate::dialect::Association::BelongsTo { default, .. } => default,
                    crate::dialect::Association::HasMany { scope, .. } => scope,
                    _ => continue,
                };
                if let Some(body) = body {
                    f(body, unlowered);
                }
            }
        }
    }
    for tm in &app.test_modules {
        if let Some(setup) = &tm.setup {
            f(setup, None);
        }
        for test in &tm.tests {
            f(&test.body, None);
        }
        for method in &tm.helpers {
            f(&method.body, None);
            for param in &method.params {
                if let Some(default) = &param.default {
                    f(default, unlowered);
                }
            }
        }
        for (_, value) in &tm.constants {
            f(value, unlowered);
        }
        for class in &tm.inner_classes {
            for (_, value) in &class.constants {
                f(value, unlowered);
            }
            for call in &class.unknown_calls {
                f(call, unlowered);
            }
            for method in &class.methods {
                f(&method.body, unlowered);
                for param in &method.params {
                    if let Some(default) = &param.default {
                        f(default, unlowered);
                    }
                }
            }
        }
    }
}

pub(crate) fn site_diagnostics(app: &App, target: Option<&str>) -> Vec<Diagnostic> {
    fn visit(
        expr: &Expr,
        classes: &ScopedClasses,
        target: Option<&str>,
        container: Option<&'static str>,
        diags: &mut Vec<Diagnostic>,
    ) {
        if let ExprNode::Send {
            recv,
            method,
            args,
            block,
            ..
        } = &*expr.node
        {
            if let Some(site) = classify_site(recv.as_ref(), method, args, block.as_ref(), classes)
            {
                if let Some((span, why)) = site
                    .refusal
                    .or_else(|| container.map(|why| (expr.span, why)))
                {
                    diags.push(Diagnostic::unsupported(
                        span,
                        target.map(Symbol::from),
                        "CurrentAttributes#set",
                        why,
                    ));
                } else if target.is_some_and(|target| !ruby_family(target)) {
                    diags.push(target_refusal(expr.span, target.expect("checked target")));
                }
            }
        }
        expr.node
            .for_each_child(&mut |child| visit(child, classes, target, container, diags));
    }
    let classes = scoped_classes(app);
    let mut diags = Vec::new();
    for_each_body(app, &mut |expr, container| {
        visit(expr, &classes, target, container, &mut diags)
    });
    diags
}

pub fn source_refusals(app: &App) -> Vec<Diagnostic> {
    site_diagnostics(app, None)
}

pub(crate) fn ruby_family(target: &str) -> bool {
    matches!(target, "ruby" | "jruby" | "spinel")
}

pub(crate) fn target_refusal(span: Span, target: &str) -> Diagnostic {
    Diagnostic::unsupported(
        span,
        Some(Symbol::from(target)),
        "CurrentAttributes#set",
        "literal scopes emit only on the Ruby-family targets; this output path remains unverified",
    )
}

pub(crate) fn set_receiver_class(
    recv: Option<&Expr>,
    method: &Symbol,
    classes: &ScopedClasses,
) -> Option<ClassId> {
    if method.as_str() != "set" {
        return None;
    }
    let recv = recv?;
    let ExprNode::Const { path } = &*recv.node else {
        return None;
    };
    if let Some(ty) = &recv.ty {
        return match ty {
            Ty::Class { id, .. } => classes.supported.contains_key(id).then(|| id.clone()),
            _ => None,
        };
    }
    let name = path
        .iter()
        .map(|p| p.as_str())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("::");
    classes
        .supported
        .keys()
        .find(|id| id.0.as_str() == name)
        .cloned()
}

pub(crate) fn keyword_pairs(args: &[Expr]) -> Option<Vec<(String, Expr)>> {
    let [opts] = args else { return None };
    let ExprNode::Hash { entries, .. } = &*opts.node else {
        return None;
    };
    if entries.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for (k, v) in entries {
        let name = match &*k.node {
            ExprNode::Lit {
                value: Literal::Sym { value },
            } => value.as_str().to_string(),
            ExprNode::Lit {
                value: Literal::Str { value },
            } => value.clone(),
            _ => return None,
        };
        out.push((name, v.clone()));
    }
    Some(out)
}
